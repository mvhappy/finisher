/* A thin shim over dav1d's decoder API (sub-project 6 spec §6.4) so the Rust side mirrors
 * none of dav1d's structs (Dav1dSettings, Dav1dData, Dav1dPicture, Dav1dSequenceHeader): it
 * passes plain values and the shim's own two records. Results are 0, FIN_DAV1D_AGAIN, or a
 * negative dav1d error. MIT, part of Finisher. */
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "dav1d/dav1d.h"

#define FIN_DAV1D_AGAIN 1

/* The values the Rust side compares against (fin-dav1d-sys `LAYOUT_*`, `CHR_*`); a negative
 * array size fails the build (portable to compilers without C11's _Static_assert). */
typedef char fin_dav1d_layouts_match[(DAV1D_PIXEL_LAYOUT_I400 == 0 && DAV1D_PIXEL_LAYOUT_I420 == 1 &&
                                      DAV1D_PIXEL_LAYOUT_I422 == 2 && DAV1D_PIXEL_LAYOUT_I444 == 3)
                                         ? 1
                                         : -1];
typedef char fin_dav1d_chroma_match[(DAV1D_CHR_UNKNOWN == 0 && DAV1D_CHR_VERTICAL == 1 &&
                                     DAV1D_CHR_COLOCATED == 2)
                                        ? 1
                                        : -1];

typedef struct FinDav1d {
    Dav1dContext* c;
} FinDav1d;

/* One decoded picture; `raw` goes back to fin_dav1d_picture_release. */
typedef struct FinDav1dPicture {
    const uint8_t* data[3];
    int64_t        stride[2];
    int32_t        width;
    int32_t        height;
    int32_t        layout;
    int32_t        bpc;
    int64_t        pts;
    /* The sequence header's colour description (CICP) and chroma siting. */
    int32_t        has_colour;
    int32_t        primaries;
    int32_t        transfer;
    int32_t        matrix;
    int32_t        full_range;
    int32_t        chroma_position;
    void*          raw;
} FinDav1dPicture;

/* What av1C needs from a sequence header OBU. */
typedef struct FinDav1dSequence {
    uint8_t profile;
    uint8_t major_level;
    uint8_t minor_level;
    uint8_t tier;
    uint8_t hbd;
    uint8_t layout;
    uint8_t chroma_position;
} FinDav1dSequence;

static int32_t status(int r) {
    return r == DAV1D_ERR(EAGAIN) ? FIN_DAV1D_AGAIN : (int32_t)r;
}

const char* fin_dav1d_version(void) {
    return dav1d_version();
}

/* `threads` workers (0: one per logical processor) with up to `frame_delay` frames in flight
 * (0: dav1d's choice, ceil(sqrt(threads)) up to 8; 1: no frame threading). Pictures larger
 * than `max_pixels` are refused (0: no limit). Only the highest spatial layer is output, and
 * dav1d's own logging is compiled out (CONFIG_LOG 0). */
int32_t fin_dav1d_open(int32_t threads, int32_t frame_delay, uint32_t max_pixels,
                       FinDav1d** out) {
    *out = NULL;
    FinDav1d* d = (FinDav1d*)calloc(1, sizeof(FinDav1d));
    if (!d) return DAV1D_ERR(ENOMEM);
    Dav1dSettings s;
    dav1d_default_settings(&s);
    s.n_threads = threads;
    s.max_frame_delay = frame_delay;
    s.frame_size_limit = max_pixels;
    s.all_layers = 0;
    s.logger.callback = NULL;
    int r = dav1d_open(&d->c, &s);
    if (r < 0) {
        free(d);
        return (int32_t)r;
    }
    *out = d;
    return 0;
}

/* The frame delay dav1d resolves for these settings. */
int32_t fin_dav1d_frame_delay(int32_t threads, int32_t frame_delay) {
    Dav1dSettings s;
    dav1d_default_settings(&s);
    s.n_threads = threads;
    s.max_frame_delay = frame_delay;
    return (int32_t)dav1d_get_frame_delay(&s);
}

/* Copies `len` bytes into a dav1d buffer stamped `pts` and sends it. FIN_DAV1D_AGAIN: not
 * taken (dav1d still holds earlier data): take a picture first and send it again. dav1d
 * 1.5 takes a packet whole or not at all. */
int32_t fin_dav1d_send(FinDav1d* d, const uint8_t* bytes, size_t len, int64_t pts) {
    if (len == 0) return DAV1D_ERR(EINVAL);
    Dav1dData data;
    memset(&data, 0, sizeof(data));
    uint8_t* buf = dav1d_data_create(&data, len);
    if (!buf) return DAV1D_ERR(ENOMEM);
    memcpy(buf, bytes, len);
    data.m.timestamp = pts;
    int r = dav1d_send_data(d->c, &data);
    /* Taken: dav1d holds its own reference and `data` is empty. Not taken or failed: release
     * ours. */
    dav1d_data_unref(&data);
    return status(r);
}

/* The next picture. FIN_DAV1D_AGAIN: none ready. Two calls in a row with no send between
 * them drain (wait for the oldest frame in flight): the caller decides when. */
int32_t fin_dav1d_get_picture(FinDav1d* d, FinDav1dPicture* out) {
    memset(out, 0, sizeof(*out));
    Dav1dPicture* p = (Dav1dPicture*)calloc(1, sizeof(Dav1dPicture));
    if (!p) return DAV1D_ERR(ENOMEM);
    int r = dav1d_get_picture(d->c, p);
    if (r < 0) {
        free(p);
        return status(r);
    }
    for (int k = 0; k < 3; k++) out->data[k] = (const uint8_t*)p->data[k];
    out->stride[0] = (int64_t)p->stride[0];
    out->stride[1] = (int64_t)p->stride[1];
    out->width = p->p.w;
    out->height = p->p.h;
    out->layout = (int32_t)p->p.layout;
    out->bpc = p->p.bpc;
    out->pts = p->m.timestamp;
    if (p->seq_hdr) {
        out->has_colour = 1;
        out->primaries = (int32_t)p->seq_hdr->pri;
        out->transfer = (int32_t)p->seq_hdr->trc;
        out->matrix = (int32_t)p->seq_hdr->mtrx;
        out->full_range = p->seq_hdr->color_range;
        out->chroma_position = (int32_t)p->seq_hdr->chr;
    }
    out->raw = p;
    return 0;
}

void fin_dav1d_picture_release(FinDav1dPicture* out) {
    Dav1dPicture* p = (Dav1dPicture*)out->raw;
    if (p) {
        dav1d_picture_unref(p);
        free(p);
    }
    memset(out, 0, sizeof(*out));
}

/* Drops buffered data and pictures (a seek). */
void fin_dav1d_flush(FinDav1d* d) {
    dav1d_flush(d->c);
}

void fin_dav1d_close(FinDav1d* d) {
    if (!d) return;
    dav1d_close(&d->c);
    free(d);
}

/* Parses the first sequence header OBU in `len` bytes. */
int32_t fin_dav1d_parse_sequence_header(const uint8_t* bytes, size_t len,
                                        FinDav1dSequence* out) {
    memset(out, 0, sizeof(*out));
    Dav1dSequenceHeader s;
    int r = dav1d_parse_sequence_header(&s, bytes, len);
    if (r < 0) return (int32_t)r;
    out->profile = (uint8_t)s.profile;
    out->major_level = (uint8_t)s.operating_points[0].major_level;
    out->minor_level = (uint8_t)s.operating_points[0].minor_level;
    out->tier = (uint8_t)s.operating_points[0].tier;
    out->hbd = (uint8_t)s.hbd;
    out->layout = (uint8_t)s.layout;
    out->chroma_position = (uint8_t)s.chr;
    return 0;
}
