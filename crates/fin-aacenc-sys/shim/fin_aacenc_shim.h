/* A thin shim over vo-aacenc's codec API (6C plan Decision 12) so the Rust side mirrors none of
 * its structs or function tables: it owns the VO_HANDLE and passes plain values. Raw AAC-LC
 * access units (no ADTS headers), 1,024 interleaved 16-bit frames in, one access unit out.
 * Every call returns 0 on success and vo-aacenc's VO_ERR_* code (as a uint32) on failure. MIT,
 * part of Finisher. */
#ifndef FIN_AACENC_SHIM_H
#define FIN_AACENC_SHIM_H

#include <stdint.h>

typedef struct FinAacEnc FinAacEnc;

/* The samples (per channel) one access unit carries. */
#define FIN_AACENC_FRAME 1024

/* The library and version this shim was built against (static). */
const char* fin_aacenc_version(void);

/* An AAC-LC encoder of `channels` (1 or 2) at `rate` Hz and `bitrate` bits a second (CBR). */
uint32_t fin_aacenc_open(int32_t rate, int32_t channels, int32_t bitrate, FinAacEnc** out);
/* Encodes FIN_AACENC_FRAME interleaved frames from `pcm` into `out` (room for `max` bytes);
 * `written` gets the access unit's length. */
uint32_t fin_aacenc_encode(FinAacEnc* e, const int16_t* pcm, uint8_t* out, int32_t max, int32_t* written);
void     fin_aacenc_close(FinAacEnc* e);

#endif
