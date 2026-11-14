/* See fin_opus_shim.h. MIT, part of Finisher. */
#include "fin_opus_shim.h"

#include <stdlib.h>

#include "opus.h"
#include "opus_multistream.h"

struct FinOpusEnc {
    OpusMSEncoder* e;
    int32_t        channels;
};

struct FinOpusDec {
    OpusMSDecoder* d;
    int32_t        channels;
};

const char* fin_opus_version(void) { return opus_get_version_string(); }

const char* fin_opus_strerror(int32_t error) { return opus_strerror(error); }

int32_t fin_opus_enc_open(int32_t channels, int32_t bitrate, FinOpusEnc** out, FinOpusEncInfo* info) {
    int            err = OPUS_OK;
    int            streams = 0, coupled = 0;
    unsigned char  mapping[8] = {0};
    opus_int32     lookahead = 0;
    FinOpusEnc*    e;
    if (channels < 1 || channels > 8 || bitrate <= 0 || !out || !info) return OPUS_BAD_ARG;
    e = (FinOpusEnc*)malloc(sizeof *e);
    if (!e) return OPUS_ALLOC_FAIL;
    e->channels = channels;
    e->e = opus_multistream_surround_encoder_create(48000, channels, channels <= 2 ? 0 : 1, &streams,
                                                    &coupled, mapping, OPUS_APPLICATION_AUDIO, &err);
    if (err != OPUS_OK || !e->e) {
        free(e);
        return err != OPUS_OK ? err : OPUS_ALLOC_FAIL;
    }
    err = opus_multistream_encoder_ctl(e->e, OPUS_SET_BITRATE(bitrate));
    if (err == OPUS_OK) err = opus_multistream_encoder_ctl(e->e, OPUS_SET_VBR(1));
    if (err == OPUS_OK) err = opus_multistream_encoder_ctl(e->e, OPUS_SET_EXPERT_FRAME_DURATION(OPUS_FRAMESIZE_20_MS));
    if (err == OPUS_OK) err = opus_multistream_encoder_ctl(e->e, OPUS_GET_LOOKAHEAD(&lookahead));
    if (err != OPUS_OK) {
        opus_multistream_encoder_destroy(e->e);
        free(e);
        return err;
    }
    info->streams = streams;
    info->coupled = coupled;
    info->lookahead = lookahead;
    for (int i = 0; i < 8; i++) info->mapping[i] = i < channels ? mapping[i] : 0;
    *out = e;
    return OPUS_OK;
}

int32_t fin_opus_enc_encode(FinOpusEnc* e, const float* pcm, int32_t frame_size, uint8_t* out, int32_t max) {
    if (!e || !pcm || !out) return OPUS_BAD_ARG;
    return opus_multistream_encode_float(e->e, pcm, frame_size, out, max);
}

void fin_opus_enc_close(FinOpusEnc* e) {
    if (!e) return;
    opus_multistream_encoder_destroy(e->e);
    free(e);
}

int32_t fin_opus_dec_open(int32_t channels, int32_t streams, int32_t coupled, const uint8_t* mapping,
                          int32_t gain, FinOpusDec** out) {
    int         err = OPUS_OK;
    FinOpusDec* d;
    if (channels < 1 || channels > 8 || !mapping || !out) return OPUS_BAD_ARG;
    d = (FinOpusDec*)malloc(sizeof *d);
    if (!d) return OPUS_ALLOC_FAIL;
    d->channels = channels;
    d->d = opus_multistream_decoder_create(48000, channels, streams, coupled, mapping, &err);
    if (err != OPUS_OK || !d->d) {
        free(d);
        return err != OPUS_OK ? err : OPUS_ALLOC_FAIL;
    }
    if (gain != 0) err = opus_multistream_decoder_ctl(d->d, OPUS_SET_GAIN(gain));
    if (err != OPUS_OK) {
        opus_multistream_decoder_destroy(d->d);
        free(d);
        return err;
    }
    *out = d;
    return OPUS_OK;
}

int32_t fin_opus_dec_decode(FinOpusDec* d, const uint8_t* data, int32_t len, float* pcm, int32_t max_frames) {
    if (!d || !data || !pcm) return OPUS_BAD_ARG;
    if (len <= 0) return OPUS_INVALID_PACKET;
    return opus_multistream_decode_float(d->d, data, len, pcm, max_frames, 0);
}

int32_t fin_opus_dec_reset(FinOpusDec* d) {
    if (!d) return OPUS_BAD_ARG;
    return opus_multistream_decoder_ctl(d->d, OPUS_RESET_STATE);
}

void fin_opus_dec_close(FinOpusDec* d) {
    if (!d) return;
    opus_multistream_decoder_destroy(d->d);
    free(d);
}
