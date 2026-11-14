/* A thin shim over libopus's multistream API (6C plan Decision 11) so the Rust side mirrors
 * none of libopus's structs and calls no variadic `_ctl`: it owns the OpusMSEncoder and
 * OpusMSDecoder and passes plain values. Every call returns 0 (or a count) on success and a
 * negative libopus error code (OPUS_BAD_ARG, OPUS_INVALID_PACKET, ...) on failure. Mono and
 * stereo are mapping family 0 (one stream), 3 to 8 channels family 1 (Vorbis order), both
 * through the multistream API. MIT, part of Finisher. */
#ifndef FIN_OPUS_SHIM_H
#define FIN_OPUS_SHIM_H

#include <stdint.h>

typedef struct FinOpusEnc FinOpusEnc;
typedef struct FinOpusDec FinOpusDec;

/* What an encoder chose: its streams, coupled streams, channel mapping and lookahead. */
typedef struct FinOpusEncInfo {
    int32_t streams;
    int32_t coupled;
    int32_t lookahead;
    uint8_t mapping[8];
} FinOpusEncInfo;

/* The linked libopus's version string (static). */
const char* fin_opus_version(void);
/* libopus's text for an error code (static). */
const char* fin_opus_strerror(int32_t error);

/* A 48 kHz OPUS_APPLICATION_AUDIO encoder of `channels` (1 to 8) at `bitrate` bits a second,
 * VBR; mapping family 0 for 1 or 2 channels, else 1. */
int32_t fin_opus_enc_open(int32_t channels, int32_t bitrate, FinOpusEnc** out, FinOpusEncInfo* info);
/* Encodes `frame_size` interleaved frames from `pcm` into `out` (at most `max` bytes); returns
 * the packet's length. */
int32_t fin_opus_enc_encode(FinOpusEnc* e, const float* pcm, int32_t frame_size, uint8_t* out, int32_t max);
void    fin_opus_enc_close(FinOpusEnc* e);

/* A 48 kHz decoder for `channels` (1 to 8) output channels from `streams` streams, `coupled`
 * of them stereo, through `mapping` (one entry per channel); `gain` is the header's output
 * gain in Q7.8 dB. */
int32_t fin_opus_dec_open(int32_t channels, int32_t streams, int32_t coupled, const uint8_t* mapping,
                          int32_t gain, FinOpusDec** out);
/* Decodes one packet into `pcm` (room for `max_frames` interleaved frames); returns the
 * frames decoded. A packet of length 0 is refused (no loss concealment here). */
int32_t fin_opus_dec_decode(FinOpusDec* d, const uint8_t* data, int32_t len, float* pcm, int32_t max_frames);
int32_t fin_opus_dec_reset(FinOpusDec* d);
void    fin_opus_dec_close(FinOpusDec* d);

#endif
