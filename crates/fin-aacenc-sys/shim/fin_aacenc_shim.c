/* See fin_aacenc_shim.h. MIT, part of Finisher. */
#include "fin_aacenc_shim.h"

#include <stdlib.h>
#include <string.h>

#include "cmnMemory.h"
#include "voAAC.h"

struct FinAacEnc {
    VO_AUDIO_CODECAPI api;
    VO_HANDLE         handle;
    VO_MEM_OPERATOR   mem;
    int32_t           channels;
};

/* FIN_AACENC_VERSION is defined by build.rs from src/version.rs (the release it recognised). */
const char* fin_aacenc_version(void) { return "vo-aacenc " FIN_AACENC_VERSION; }

uint32_t fin_aacenc_open(int32_t rate, int32_t channels, int32_t bitrate, FinAacEnc** out) {
    VO_CODEC_INIT_USERDATA user;
    AACENC_PARAM           params;
    FinAacEnc*             e;
    uint32_t               r;
    if (!out || channels < 1 || channels > 2 || rate <= 0 || bitrate <= 0) return VO_ERR_INVALID_ARG;
    *out = NULL;
    e = (FinAacEnc*)calloc(1, sizeof *e);
    if (!e) return VO_ERR_OUTOF_MEMORY;
    e->channels = channels;
    r = voGetAACEncAPI(&e->api);
    if (r != VO_ERR_NONE) {
        free(e);
        return r;
    }
    e->mem.Alloc = cmnMemAlloc;
    e->mem.Copy = cmnMemCopy;
    e->mem.Free = cmnMemFree;
    e->mem.Set = cmnMemSet;
    e->mem.Check = cmnMemCheck;
    memset(&user, 0, sizeof user);
    user.memflag = VO_IMF_USERMEMOPERATOR;
    user.memData = &e->mem;
    r = e->api.Init(&e->handle, VO_AUDIO_CodingAAC, &user);
    if (r != VO_ERR_NONE || !e->handle) {
        free(e);
        return r != VO_ERR_NONE ? r : VO_ERR_OUTOF_MEMORY;
    }
    memset(&params, 0, sizeof params);
    params.sampleRate = rate;
    params.bitRate = bitrate;
    params.nChannels = (short)channels;
    params.adtsUsed = 0;
    r = e->api.SetParam(e->handle, VO_PID_AAC_ENCPARAM, &params);
    if (r != VO_ERR_NONE) {
        e->api.Uninit(e->handle);
        free(e);
        return r;
    }
    *out = e;
    return VO_ERR_NONE;
}

uint32_t fin_aacenc_encode(FinAacEnc* e, const int16_t* pcm, uint8_t* out, int32_t max, int32_t* written) {
    VO_CODECBUFFER      input, output;
    VO_AUDIO_OUTPUTINFO info;
    uint32_t            r;
    if (!e || !pcm || !out || max <= 0 || !written) return VO_ERR_INVALID_ARG;
    *written = 0;
    memset(&input, 0, sizeof input);
    memset(&output, 0, sizeof output);
    memset(&info, 0, sizeof info);
    /* The library reads the input through a non-const pointer but does not write it. */
    input.Buffer = (VO_PBYTE)(uintptr_t)pcm;
    input.Length = (VO_U32)(FIN_AACENC_FRAME * e->channels * sizeof(int16_t));
    r = e->api.SetInputData(e->handle, &input);
    if (r != VO_ERR_NONE) return r;
    output.Buffer = out;
    output.Length = (VO_U32)max;
    r = e->api.GetOutputData(e->handle, &output, &info);
    if (r != VO_ERR_NONE) return r;
    *written = (int32_t)output.Length;
    return VO_ERR_NONE;
}

void fin_aacenc_close(FinAacEnc* e) {
    if (!e) return;
    e->api.Uninit(e->handle);
    free(e);
}
