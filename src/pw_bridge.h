#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PSM_MAX_PLANES 4
#define PSM_MAX_MODIFIERS_PER_FMT 64
#define PSM_MAX_DRM_FORMATS 8

enum PsmPixelFormat {
    PSM_FMT_UNKNOWN = 0,
    PSM_FMT_RGBA = 1,
    PSM_FMT_RGBX = 2,
    PSM_FMT_BGRA = 3,
    PSM_FMT_BGRX = 4,
};

struct PsmDrmFormatModifiers {
    uint32_t drm_format;
    uint32_t spa_format_kind; /* PsmPixelFormat */
    uint32_t modifier_count;
    uint64_t modifiers[PSM_MAX_MODIFIERS_PER_FMT];
};

struct PsmFrame {
    bool is_dmabuf;
    uint32_t width;
    uint32_t height;
    uint32_t format;      /* PsmPixelFormat */
    uint32_t drm_format;  /* DRM fourcc when is_dmabuf */
    uint64_t modifier;    /* DRM modifier when is_dmabuf */
    bool y_invert;

    /* Valid when is_dmabuf == true; caller owns FDs after pop and must close them */
    uint32_t plane_count;
    int32_t plane_fds[PSM_MAX_PLANES];
    uint32_t plane_offsets[PSM_MAX_PLANES];
    uint32_t plane_strides[PSM_MAX_PLANES];

    /* Valid when is_dmabuf == false; points into internal read slot valid until next pop or stop */
    const uint8_t *shm_pixels;
    uint32_t shm_stride;
    size_t shm_size;

    uint64_t seq;
};

typedef struct PsmPwStream PsmPwStream;

PsmPwStream *psm_pw_start(
    int pw_fd,
    uint32_t node_id,
    int wakeup_eventfd,
    bool allow_dmabuf,
    const struct PsmDrmFormatModifiers *drm_formats,
    size_t drm_format_count,
    uint32_t target_fps
);

bool psm_pw_pop_frame(PsmPwStream *s, struct PsmFrame *out_frame);
void psm_pw_force_shm(PsmPwStream *s);
void psm_pw_set_fps(PsmPwStream *s, uint32_t target_fps);
bool psm_pw_has_error(PsmPwStream *s, char *err_buf, size_t err_buf_len);
void psm_pw_stop(PsmPwStream *s);

#ifdef __cplusplus
}
#endif
