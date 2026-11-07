/* A thin shim over SVT-AV1's encoder API (sub-project 6 spec §6.4) so the Rust side never
 * mirrors SVT-AV1's large, version-specific configuration struct: the shim owns it, and
 * fields are set by name through svt_av1_enc_parse_parameter. Only the encoder calls the plan
 * names are used. MIT, part of Finisher. */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "EbSvtAv1Enc.h"

typedef struct FinSvt {
    EbComponentType*         handle;
    EbSvtAv1EncConfiguration cfg;
    int                      initialised;
} FinSvt;

/* One output packet; `raw` goes back to fin_svt_release. */
typedef struct FinSvtPacket {
    const uint8_t* data;
    uint32_t       len;
    int64_t        pts;
    int64_t        dts;
    uint32_t       flags;
    uint32_t       pic_type;
    void*          raw;
} FinSvtPacket;

FinSvt* fin_svt_new(int32_t* err) {
    FinSvt* s = (FinSvt*)calloc(1, sizeof(FinSvt));
    if (!s) {
        *err = (int32_t)EB_ErrorInsufficientResources;
        return NULL;
    }
    EbErrorType e = svt_av1_enc_init_handle(&s->handle, &s->cfg);
    if (e != EB_ErrorNone) {
        free(s);
        *err = (int32_t)e;
        return NULL;
    }
    *err = 0;
    return s;
}

int32_t fin_svt_param(FinSvt* s, const char* name, const char* value) {
    return (int32_t)svt_av1_enc_parse_parameter(&s->cfg, name, value);
}

int32_t fin_svt_init(FinSvt* s) {
    EbErrorType e = svt_av1_enc_set_parameter(s->handle, &s->cfg);
    if (e != EB_ErrorNone) return (int32_t)e;
    e = svt_av1_enc_init(s->handle);
    if (e == EB_ErrorNone) s->initialised = 1;
    return (int32_t)e;
}

/* Copies the stream header (the sequence header OBU) into out; *len gets its size. */
int32_t fin_svt_header(FinSvt* s, uint8_t* out, uint32_t cap, uint32_t* len) {
    EbBufferHeaderType* h = NULL;
    EbErrorType         e = svt_av1_enc_stream_header(s->handle, &h);
    if (e != EB_ErrorNone) return (int32_t)e;
    *len = h->n_filled_len;
    if (h->n_filled_len <= cap) memcpy(out, h->p_buffer, h->n_filled_len);
    svt_av1_enc_stream_header_release(h);
    return 0;
}

/* Sends one 8-bit 4:2:0 picture, or end of stream when y is NULL. */
int32_t fin_svt_send(FinSvt* s, const uint8_t* y, const uint8_t* u, const uint8_t* v, uint32_t y_stride,
                     uint32_t uv_stride, uint32_t luma_bytes, int64_t pts, int32_t force_key) {
    EbSvtIOFormat      io;
    EbBufferHeaderType b;
    memset(&io, 0, sizeof(io));
    memset(&b, 0, sizeof(b));
    b.size = sizeof(b);
    if (!y) {
        b.flags = EB_BUFFERFLAG_EOS;
        b.pic_type = EB_AV1_INVALID_PICTURE;
        return (int32_t)svt_av1_enc_send_picture(s->handle, &b);
    }
    io.luma = (uint8_t*)y;
    io.cb = (uint8_t*)u;
    io.cr = (uint8_t*)v;
    io.y_stride = y_stride;
    io.cb_stride = uv_stride;
    io.cr_stride = uv_stride;
    b.p_buffer = (uint8_t*)&io;
    b.n_filled_len = luma_bytes + 2 * (luma_bytes / 4);
    b.n_alloc_len = b.n_filled_len;
    b.pts = pts;
    b.pic_type = force_key ? EB_AV1_KEY_PICTURE : EB_AV1_INVALID_PICTURE;
    return (int32_t)svt_av1_enc_send_picture(s->handle, &b);
}

/* 0: a packet in *out; 1: none yet; otherwise an SVT-AV1 error. */
int32_t fin_svt_packet(FinSvt* s, uint8_t done, FinSvtPacket* out) {
    EbBufferHeaderType* b = NULL;
    EbErrorType         e = svt_av1_enc_get_packet(s->handle, &b, done);
    if (e == EB_NoErrorEmptyQueue) return 1;
    if (e != EB_ErrorNone) return (int32_t)e;
    out->data = b->p_buffer;
    out->len = b->n_filled_len;
    out->pts = b->pts;
    out->dts = b->dts;
    out->flags = b->flags;
    out->pic_type = (uint32_t)b->pic_type;
    out->raw = b;
    return 0;
}

void fin_svt_release(FinSvtPacket* p) {
    EbBufferHeaderType* b = (EbBufferHeaderType*)p->raw;
    if (b) svt_av1_enc_release_out_buffer(&b);
    p->raw = NULL;
}

void fin_svt_free(FinSvt* s) {
    if (!s) return;
    if (s->initialised) svt_av1_enc_deinit(s->handle);
    svt_av1_enc_deinit_handle(s->handle);
    free(s);
}
