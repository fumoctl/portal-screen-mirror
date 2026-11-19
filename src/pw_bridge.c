#define _GNU_SOURCE
#include "pw_bridge.h"

#include <errno.h>
#include <fcntl.h>
#include <pipewire/pipewire.h>
#include <pthread.h>
#include <spa/buffer/buffer.h>
#include <spa/buffer/meta.h>
#include <spa/param/param.h>
#if defined(__has_include) && __has_include(<spa/param/buffers.h>)
#include <spa/param/buffers.h>
#endif
#include <spa/param/format-utils.h>
#include <spa/param/video/format-utils.h>
#include <spa/pod/builder.h>
#include <spa/pod/iter.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#ifndef DRM_FORMAT_ARGB8888
#define DRM_FORMAT_ARGB8888 0x34325241u
#define DRM_FORMAT_XRGB8888 0x34325258u
#define DRM_FORMAT_ABGR8888 0x34324241u
#define DRM_FORMAT_XBGR8888 0x34324258u
#endif

/* PipeWire < 0.3.60 (e.g. Ubuntu 22.04 baseline) predates SPA_META_VideoTransform in headers */
#if !PW_CHECK_VERSION(0, 3, 60)
#define SPA_META_VideoTransform 8u
#define SPA_META_TRANSFORMATION_Flipped 4u
#define SPA_META_TRANSFORMATION_Flipped180 6u
struct spa_meta_videotransform {
    uint32_t transform;
};
#endif

struct ShmSlot {
    uint8_t *data;
    size_t cap;
    size_t len;
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t format;
    bool y_invert;
};

struct DmaSlot {
    bool valid;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t drm_format;
    uint64_t modifier;
    bool y_invert;
    uint32_t plane_count;
    int32_t fds[PSM_MAX_PLANES];
    uint32_t offsets[PSM_MAX_PLANES];
    uint32_t strides[PSM_MAX_PLANES];
};

struct PsmPwStream {
    struct pw_thread_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_stream *stream;
    struct spa_hook core_listener;
    struct spa_hook stream_listener;

    int wakeup_fd;
    uint32_t node_id;
    bool allow_dmabuf;
    uint32_t target_fps;
    size_t drm_count;
    struct PsmDrmFormatModifiers drm_formats[PSM_MAX_DRM_FORMATS];

    struct spa_video_info_raw video_info;
    bool negotiated_dmabuf;

    pthread_mutex_t mtx;
    bool has_error;
    char error_msg[256];

    uint64_t latest_seq;
    uint64_t popped_seq;
    bool latest_is_dmabuf;

    struct DmaSlot pending_dma;

    struct ShmSlot shm_slots[3];
    int consumer_slot;
    int ready_slot;
};

static void notify_wakeup(const PsmPwStream *s) {
    if (s->wakeup_fd >= 0) {
        uint64_t one = 1;
        ssize_t r = write(s->wakeup_fd, &one, sizeof(one));
        (void)r;
    }
}

static uint32_t psm_fmt_to_spa(uint32_t fmt) {
    switch (fmt) {
    case PSM_FMT_RGBA:
        return SPA_VIDEO_FORMAT_RGBA;
    case PSM_FMT_RGBX:
        return SPA_VIDEO_FORMAT_RGBx;
    case PSM_FMT_BGRA:
        return SPA_VIDEO_FORMAT_BGRA;
    case PSM_FMT_BGRX:
    default:
        return SPA_VIDEO_FORMAT_BGRx;
    }
}

static uint32_t spa_fmt_to_psm(uint32_t spa_fmt) {
    switch (spa_fmt) {
    case SPA_VIDEO_FORMAT_RGBA:
        return PSM_FMT_RGBA;
    case SPA_VIDEO_FORMAT_RGBx:
        return PSM_FMT_RGBX;
    case SPA_VIDEO_FORMAT_BGRA:
        return PSM_FMT_BGRA;
    case SPA_VIDEO_FORMAT_BGRx:
        return PSM_FMT_BGRX;
    default:
        return PSM_FMT_UNKNOWN;
    }
}

static uint32_t spa_fmt_to_drm(uint32_t spa_fmt) {
    switch (spa_fmt) {
    case SPA_VIDEO_FORMAT_RGBA:
        return DRM_FORMAT_ABGR8888;
    case SPA_VIDEO_FORMAT_RGBx:
        return DRM_FORMAT_XBGR8888;
    case SPA_VIDEO_FORMAT_BGRA:
        return DRM_FORMAT_ARGB8888;
    case SPA_VIDEO_FORMAT_BGRx:
    default:
        return DRM_FORMAT_XRGB8888;
    }
}

static void close_dma_slot(struct DmaSlot *slot) {
    if (!slot->valid) {
        return;
    }
    for (uint32_t i = 0; i < slot->plane_count; i++) {
        if (slot->fds[i] >= 0) {
            close(slot->fds[i]);
            slot->fds[i] = -1;
        }
    }
    slot->plane_count = 0;
    slot->valid = false;
}

static size_t build_enum_formats(
    PsmPwStream *s,
    struct spa_pod_builder *b,
    const struct spa_pod **params,
    size_t max_params
) {
    size_t count = 0;
    uint32_t max_fps = (s->target_fps > 0 && s->target_fps <= 500) ? s->target_fps : 240;

    struct spa_rectangle def_size = SPA_RECTANGLE(1920, 1080);
    struct spa_rectangle min_size = SPA_RECTANGLE(2, 2);
    struct spa_rectangle max_size = SPA_RECTANGLE(16384, 16384);
    struct spa_fraction var_fps = SPA_FRACTION(0, 1);
    struct spa_fraction min_fps = SPA_FRACTION(0, 1);
    struct spa_fraction cap_fps = SPA_FRACTION(max_fps, 1);

    if (s->allow_dmabuf) {
        for (size_t i = 0; i < s->drm_count && count + 1 < max_params; i++) {
            const struct PsmDrmFormatModifiers *df = &s->drm_formats[i];
            if (df->modifier_count == 0) {
                continue;
            }
            uint32_t spa_fmt = psm_fmt_to_spa(df->spa_format_kind);
            struct spa_pod_frame outer;
            struct spa_pod_frame mod_choice;

            spa_pod_builder_push_object(b, &outer, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
            spa_pod_builder_add(
                b,
                SPA_FORMAT_mediaType,
                SPA_POD_Id(SPA_MEDIA_TYPE_video),
                SPA_FORMAT_mediaSubtype,
                SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
                SPA_FORMAT_VIDEO_format,
                SPA_POD_Id(spa_fmt),
                0
            );

            spa_pod_builder_prop(
                b,
                SPA_FORMAT_VIDEO_modifier,
                SPA_POD_PROP_FLAG_MANDATORY | SPA_POD_PROP_FLAG_DONT_FIXATE
            );
            spa_pod_builder_push_choice(b, &mod_choice, SPA_CHOICE_Enum, 0);
            spa_pod_builder_long(b, (int64_t)df->modifiers[0]);
            for (uint32_t m = 0; m < df->modifier_count; m++) {
                spa_pod_builder_long(b, (int64_t)df->modifiers[m]);
            }
            spa_pod_builder_pop(b, &mod_choice);

            spa_pod_builder_add(
                b,
                SPA_FORMAT_VIDEO_size,
                SPA_POD_CHOICE_RANGE_Rectangle(&def_size, &min_size, &max_size),
                SPA_FORMAT_VIDEO_framerate,
                SPA_POD_CHOICE_RANGE_Fraction(&var_fps, &min_fps, &cap_fps),
                SPA_FORMAT_VIDEO_maxFramerate,
                SPA_POD_CHOICE_RANGE_Fraction(&cap_fps, &min_fps, &cap_fps),
                0
            );
            params[count++] = (const struct spa_pod *)spa_pod_builder_pop(b, &outer);
        }
    }

    if (count < max_params) {
        struct spa_pod_frame outer;
        spa_pod_builder_push_object(b, &outer, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
        spa_pod_builder_add(
            b,
            SPA_FORMAT_mediaType,
            SPA_POD_Id(SPA_MEDIA_TYPE_video),
            SPA_FORMAT_mediaSubtype,
            SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
            SPA_FORMAT_VIDEO_format,
            SPA_POD_CHOICE_ENUM_Id(
                5,
                SPA_VIDEO_FORMAT_BGRx,
                SPA_VIDEO_FORMAT_BGRx,
                SPA_VIDEO_FORMAT_BGRA,
                SPA_VIDEO_FORMAT_RGBx,
                SPA_VIDEO_FORMAT_RGBA
            ),
            SPA_FORMAT_VIDEO_size,
            SPA_POD_CHOICE_RANGE_Rectangle(&def_size, &min_size, &max_size),
            SPA_FORMAT_VIDEO_framerate,
            SPA_POD_CHOICE_RANGE_Fraction(&var_fps, &min_fps, &cap_fps),
            SPA_FORMAT_VIDEO_maxFramerate,
            SPA_POD_CHOICE_RANGE_Fraction(&cap_fps, &min_fps, &cap_fps),
            0
        );
        params[count++] = (const struct spa_pod *)spa_pod_builder_pop(b, &outer);
    }

    return count;
}

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    (void)id;
    (void)seq;
    PsmPwStream *s = (PsmPwStream *)data;
    if (res == -EPIPE) {
        pthread_mutex_lock(&s->mtx);
        s->has_error = true;
        snprintf(s->error_msg, sizeof(s->error_msg), "PipeWire remote closed: %s", message ? message : "EPIPE");
        pthread_mutex_unlock(&s->mtx);
        notify_wakeup(s);
    }
}

static const struct pw_core_events CORE_EVENTS = {
    PW_VERSION_CORE_EVENTS,
    .error = on_core_error,
};

static void on_state_changed(void *data, enum pw_stream_state old, enum pw_stream_state state, const char *error) {
    (void)old;
    PsmPwStream *s = (PsmPwStream *)data;
    if (state == PW_STREAM_STATE_ERROR) {
        pthread_mutex_lock(&s->mtx);
        s->has_error = true;
        snprintf(s->error_msg, sizeof(s->error_msg), "%s", error ? error : "PipeWire stream error");
        pthread_mutex_unlock(&s->mtx);
        notify_wakeup(s);
    }
}

static void on_param_changed(void *data, uint32_t id, const struct spa_pod *param) {
    PsmPwStream *s = (PsmPwStream *)data;
    if (id != SPA_PARAM_Format || param == NULL) {
        return;
    }

    struct spa_video_info_raw info;
    memset(&info, 0, sizeof(info));
    if (spa_format_video_raw_parse(param, &info) < 0) {
        return;
    }
    s->video_info = info;
    bool has_mod = spa_pod_find_prop(param, NULL, SPA_FORMAT_VIDEO_modifier) != NULL;
    s->negotiated_dmabuf = has_mod && s->allow_dmabuf;

    uint32_t data_types = s->negotiated_dmabuf
        ? (1u << SPA_DATA_DmaBuf)
        : ((1u << SPA_DATA_MemFd) | (1u << SPA_DATA_MemPtr));

    uint8_t buf[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
    const struct spa_pod *params[4];
    uint32_t n = 0;

    params[n++] = (const struct spa_pod *)spa_pod_builder_add_object(
        &b,
        SPA_TYPE_OBJECT_ParamBuffers,
        SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers,
        SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_dataType,
        SPA_POD_CHOICE_FLAGS_Int(data_types)
    );
    params[n++] = (const struct spa_pod *)spa_pod_builder_add_object(
        &b,
        SPA_TYPE_OBJECT_ParamMeta,
        SPA_PARAM_Meta,
        SPA_PARAM_META_type,
        SPA_POD_Id(SPA_META_Header),
        SPA_PARAM_META_size,
        SPA_POD_Int((int32_t)sizeof(struct spa_meta_header))
    );
    params[n++] = (const struct spa_pod *)spa_pod_builder_add_object(
        &b,
        SPA_TYPE_OBJECT_ParamMeta,
        SPA_PARAM_Meta,
        SPA_PARAM_META_type,
        SPA_POD_Id(SPA_META_VideoTransform),
        SPA_PARAM_META_size,
        SPA_POD_Int((int32_t)sizeof(struct spa_meta_videotransform))
    );

    pw_stream_update_params(s->stream, params, n);
}

static void on_process(void *data) {
    PsmPwStream *s = (PsmPwStream *)data;
    struct pw_buffer *pwb = NULL;

    /* Drain to the freshest queued buffer if multiple accumulated */
    while (true) {
        struct pw_buffer *next = pw_stream_dequeue_buffer(s->stream);
        if (next == NULL) {
            break;
        }
        if (pwb != NULL) {
            pw_stream_queue_buffer(s->stream, pwb);
        }
        pwb = next;
    }
    if (pwb == NULL) {
        return;
    }

    struct spa_buffer *buf = pwb->buffer;
    uint32_t w = s->video_info.size.width;
    uint32_t h = s->video_info.size.height;
    uint32_t psm_fmt = spa_fmt_to_psm(s->video_info.format);

    struct spa_meta_header *hdr =
        (struct spa_meta_header *)spa_buffer_find_meta_data(buf, SPA_META_Header, sizeof(*hdr));
    if (hdr && (hdr->flags & SPA_META_HEADER_FLAG_CORRUPTED)) {
        pw_stream_queue_buffer(s->stream, pwb);
        return;
    }

    bool y_invert = false;
    struct spa_meta_videotransform *vt =
        (struct spa_meta_videotransform *)spa_buffer_find_meta_data(buf, SPA_META_VideoTransform, sizeof(*vt));
    if (vt && (vt->transform == SPA_META_TRANSFORMATION_Flipped || vt->transform == SPA_META_TRANSFORMATION_Flipped180)) {
        y_invert = true;
    }

    if (w == 0 || h == 0 || psm_fmt == PSM_FMT_UNKNOWN || buf->n_datas == 0 || buf->datas[0].chunk == NULL) {
        pw_stream_queue_buffer(s->stream, pwb);
        return;
    }

    if (buf->datas[0].type == SPA_DATA_DmaBuf && s->allow_dmabuf) {
        uint32_t n_planes = buf->n_datas > PSM_MAX_PLANES ? PSM_MAX_PLANES : buf->n_datas;
        bool broken = false;

        /* Catch malformed multi-plane portal buffers (e.g. chunk->size == 9 DCC bug) immediately */
        if (buf->datas[0].chunk->size == 0 || (n_planes > 1 && buf->datas[0].chunk->size < 64)) {
            broken = true;
        }
        for (uint32_t i = 0; i < n_planes && !broken; i++) {
            if (buf->datas[i].fd < 0 || buf->datas[i].chunk == NULL) {
                broken = true;
            }
        }

        if (broken) {
            pw_stream_queue_buffer(s->stream, pwb);
            s->allow_dmabuf = false;
            uint8_t pbuf[8192];
            struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(pbuf, sizeof(pbuf));
            const struct spa_pod *params[16];
            size_t cnt = build_enum_formats(s, &pb, params, 16);
            if (cnt > 0) {
                pw_stream_update_params(s->stream, params, (uint32_t)cnt);
            }
            return;
        }

        int32_t dup_fds[PSM_MAX_PLANES] = {-1, -1, -1, -1};
        uint32_t offsets[PSM_MAX_PLANES] = {0};
        uint32_t strides[PSM_MAX_PLANES] = {0};
        bool dup_ok = true;

        for (uint32_t i = 0; i < n_planes; i++) {
            int dfd = fcntl((int)buf->datas[i].fd, F_DUPFD_CLOEXEC, 0);
            if (dfd < 0) {
                dup_ok = false;
                break;
            }
            dup_fds[i] = dfd;
            offsets[i] = buf->datas[i].chunk->offset;
            strides[i] = (uint32_t)buf->datas[i].chunk->stride;
        }

        if (!dup_ok) {
            for (uint32_t i = 0; i < n_planes; i++) {
                if (dup_fds[i] >= 0) {
                    close(dup_fds[i]);
                }
            }
            pw_stream_queue_buffer(s->stream, pwb);
            return;
        }

        pthread_mutex_lock(&s->mtx);
        close_dma_slot(&s->pending_dma);
        s->pending_dma.valid = true;
        s->pending_dma.width = w;
        s->pending_dma.height = h;
        s->pending_dma.format = psm_fmt;
        s->pending_dma.drm_format = spa_fmt_to_drm(s->video_info.format);
        s->pending_dma.modifier = s->video_info.modifier;
        s->pending_dma.y_invert = y_invert;
        s->pending_dma.plane_count = n_planes;
        for (uint32_t i = 0; i < n_planes; i++) {
            s->pending_dma.fds[i] = dup_fds[i];
            s->pending_dma.offsets[i] = offsets[i];
            s->pending_dma.strides[i] = strides[i];
        }
        s->latest_is_dmabuf = true;
        s->latest_seq++;
        pthread_mutex_unlock(&s->mtx);

        pw_stream_queue_buffer(s->stream, pwb);
        notify_wakeup(s);
        return;
    }

    /* SHM path (SPA_DATA_MemFd / SPA_DATA_MemPtr) */
    struct spa_data *d = &buf->datas[0];
    uint32_t src_stride = d->chunk->stride > 0 ? (uint32_t)d->chunk->stride : (w * 4);
    uint32_t dst_stride = w * 4;
    size_t needed = (size_t)dst_stride * (size_t)h;

    if (src_stride < dst_stride || d->chunk->size < (size_t)src_stride * (h - 1) + dst_stride) {
        pw_stream_queue_buffer(s->stream, pwb);
        return;
    }

    const uint8_t *src_ptr = NULL;
    void *mapped_base = MAP_FAILED;
    size_t mapped_len = 0;

    if (d->data != NULL) {
        src_ptr = (const uint8_t *)d->data + d->chunk->offset;
    } else if (d->type == SPA_DATA_MemFd && d->fd >= 0) {
        long page_sz = sysconf(_SC_PAGESIZE);
        if (page_sz <= 0) {
            page_sz = 4096;
        }
        size_t align_mask = (size_t)page_sz - 1;
        size_t map_off = (size_t)d->chunk->offset & ~align_mask;
        size_t delta = (size_t)d->chunk->offset - map_off;
        mapped_len = delta + (size_t)src_stride * (size_t)h;
        mapped_base = mmap(NULL, mapped_len, PROT_READ, MAP_SHARED, (int)d->fd, (off_t)map_off);
        if (mapped_base != MAP_FAILED) {
            src_ptr = (const uint8_t *)mapped_base + delta;
        }
    }

    if (src_ptr == NULL) {
        pw_stream_queue_buffer(s->stream, pwb);
        return;
    }

    pthread_mutex_lock(&s->mtx);
    int write_idx = 0;
    for (int i = 0; i < 3; i++) {
        if (i != s->consumer_slot && i != s->ready_slot) {
            write_idx = i;
            break;
        }
    }
    struct ShmSlot *slot = &s->shm_slots[write_idx];
    pthread_mutex_unlock(&s->mtx);

    if (slot->cap < needed) {
        uint8_t *nb = (uint8_t *)realloc(slot->data, needed);
        if (nb == NULL) {
            if (mapped_base != MAP_FAILED) {
                munmap(mapped_base, mapped_len);
            }
            pw_stream_queue_buffer(s->stream, pwb);
            return;
        }
        slot->data = nb;
        slot->cap = needed;
    }

    if (src_stride == dst_stride) {
        memcpy(slot->data, src_ptr, needed);
    } else {
        for (uint32_t y = 0; y < h; y++) {
            memcpy(slot->data + (size_t)y * dst_stride, src_ptr + (size_t)y * src_stride, dst_stride);
        }
    }
    slot->len = needed;
    slot->width = w;
    slot->height = h;
    slot->stride = dst_stride;
    slot->format = psm_fmt;
    slot->y_invert = y_invert;

    if (mapped_base != MAP_FAILED) {
        munmap(mapped_base, mapped_len);
    }
    pw_stream_queue_buffer(s->stream, pwb);

    pthread_mutex_lock(&s->mtx);
    close_dma_slot(&s->pending_dma);
    s->ready_slot = write_idx;
    s->latest_is_dmabuf = false;
    s->latest_seq++;
    pthread_mutex_unlock(&s->mtx);

    notify_wakeup(s);
}

static const struct pw_stream_events STREAM_EVENTS = {
    PW_VERSION_STREAM_EVENTS,
    .state_changed = on_state_changed,
    .param_changed = on_param_changed,
    .process = on_process,
};

PsmPwStream *psm_pw_start(
    int pw_fd,
    uint32_t node_id,
    int wakeup_eventfd,
    bool allow_dmabuf,
    const struct PsmDrmFormatModifiers *drm_formats,
    size_t drm_format_count,
    uint32_t target_fps
) {
    static pthread_once_t pw_once = PTHREAD_ONCE_INIT;
    pthread_once(&pw_once, (void (*)(void))pw_init);

    int dup_pw_fd = fcntl(pw_fd, F_DUPFD_CLOEXEC, 0);
    if (dup_pw_fd < 0) {
        return NULL;
    }

    PsmPwStream *s = (PsmPwStream *)calloc(1, sizeof(PsmPwStream));
    if (!s) {
        close(dup_pw_fd);
        return NULL;
    }

    pthread_mutex_init(&s->mtx, NULL);
    s->wakeup_fd = wakeup_eventfd;
    s->node_id = node_id;
    s->allow_dmabuf = allow_dmabuf && drm_format_count > 0;
    s->target_fps = target_fps;
    s->consumer_slot = -1;
    s->ready_slot = -1;
    for (int i = 0; i < PSM_MAX_PLANES; i++) {
        s->pending_dma.fds[i] = -1;
    }
    if (drm_formats && drm_format_count > 0) {
        s->drm_count = drm_format_count > PSM_MAX_DRM_FORMATS ? PSM_MAX_DRM_FORMATS : drm_format_count;
        memcpy(s->drm_formats, drm_formats, s->drm_count * sizeof(struct PsmDrmFormatModifiers));
    }

    s->loop = pw_thread_loop_new("psm-pipewire", NULL);
    if (!s->loop) {
        close(dup_pw_fd);
        pthread_mutex_destroy(&s->mtx);
        free(s);
        return NULL;
    }

    pw_thread_loop_lock(s->loop);
    s->context = pw_context_new(pw_thread_loop_get_loop(s->loop), NULL, 0);
    if (!s->context) {
        close(dup_pw_fd);
        pw_thread_loop_unlock(s->loop);
        pw_thread_loop_destroy(s->loop);
        pthread_mutex_destroy(&s->mtx);
        free(s);
        return NULL;
    }

    s->core = pw_context_connect_fd(s->context, dup_pw_fd, NULL, 0);
    if (!s->core) {
        pw_context_destroy(s->context);
        pw_thread_loop_unlock(s->loop);
        pw_thread_loop_destroy(s->loop);
        pthread_mutex_destroy(&s->mtx);
        free(s);
        return NULL;
    }
    pw_core_add_listener(s->core, &s->core_listener, &CORE_EVENTS, s);

    struct pw_properties *props = pw_properties_new(
        PW_KEY_MEDIA_TYPE,
        "Video",
        PW_KEY_MEDIA_CATEGORY,
        "Capture",
        PW_KEY_MEDIA_ROLE,
        "Screen",
        NULL
    );
    s->stream = pw_stream_new(s->core, "portal-screen-mirror", props);
    if (!s->stream) {
        pw_core_disconnect(s->core);
        pw_context_destroy(s->context);
        pw_thread_loop_unlock(s->loop);
        pw_thread_loop_destroy(s->loop);
        pthread_mutex_destroy(&s->mtx);
        free(s);
        return NULL;
    }
    pw_stream_add_listener(s->stream, &s->stream_listener, &STREAM_EVENTS, s);

    uint8_t pbuf[16384];
    struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(pbuf, sizeof(pbuf));
    const struct spa_pod *params[16];
    size_t n_params = build_enum_formats(s, &pb, params, 16);

    /* Intentionally omit PW_STREAM_FLAG_MAP_BUFFERS so libpipewire never attempts mmap() on tiled GPU DMA-BUFs */
    int rc = pw_stream_connect(
        s->stream,
        PW_DIRECTION_INPUT,
        node_id,
        (enum pw_stream_flags)(PW_STREAM_FLAG_AUTOCONNECT),
        params,
        (uint32_t)n_params
    );
    pw_thread_loop_unlock(s->loop);

    if (rc < 0 || pw_thread_loop_start(s->loop) < 0) {
        psm_pw_stop(s);
        return NULL;
    }
    return s;
}

bool psm_pw_pop_frame(PsmPwStream *s, struct PsmFrame *out_frame) {
    if (!s || !out_frame) {
        return false;
    }
    memset(out_frame, 0, sizeof(*out_frame));

    pthread_mutex_lock(&s->mtx);
    if (s->latest_seq == 0 || s->latest_seq <= s->popped_seq) {
        pthread_mutex_unlock(&s->mtx);
        return false;
    }

    if (s->latest_is_dmabuf && s->pending_dma.valid) {
        out_frame->is_dmabuf = true;
        out_frame->width = s->pending_dma.width;
        out_frame->height = s->pending_dma.height;
        out_frame->format = s->pending_dma.format;
        out_frame->drm_format = s->pending_dma.drm_format;
        out_frame->modifier = s->pending_dma.modifier;
        out_frame->y_invert = s->pending_dma.y_invert;
        out_frame->plane_count = s->pending_dma.plane_count;
        for (uint32_t i = 0; i < s->pending_dma.plane_count; i++) {
            out_frame->plane_fds[i] = s->pending_dma.fds[i];
            out_frame->plane_offsets[i] = s->pending_dma.offsets[i];
            out_frame->plane_strides[i] = s->pending_dma.strides[i];
            s->pending_dma.fds[i] = -1;
        }
        s->pending_dma.valid = false;
        out_frame->seq = s->latest_seq;
        s->popped_seq = s->latest_seq;
        pthread_mutex_unlock(&s->mtx);
        return true;
    }

    if (!s->latest_is_dmabuf && s->ready_slot >= 0) {
        int idx = s->ready_slot;
        s->consumer_slot = idx;
        struct ShmSlot *slot = &s->shm_slots[idx];
        out_frame->is_dmabuf = false;
        out_frame->width = slot->width;
        out_frame->height = slot->height;
        out_frame->format = slot->format;
        out_frame->y_invert = slot->y_invert;
        out_frame->shm_pixels = slot->data;
        out_frame->shm_stride = slot->stride;
        out_frame->shm_size = slot->len;
        out_frame->seq = s->latest_seq;
        s->popped_seq = s->latest_seq;
        pthread_mutex_unlock(&s->mtx);
        return true;
    }

    pthread_mutex_unlock(&s->mtx);
    return false;
}

void psm_pw_force_shm(PsmPwStream *s) {
    if (!s || !s->loop || !s->stream) {
        return;
    }
    pw_thread_loop_lock(s->loop);
    if (s->allow_dmabuf) {
        s->allow_dmabuf = false;
        uint8_t pbuf[4096];
        struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(pbuf, sizeof(pbuf));
        const struct spa_pod *params[4];
        size_t cnt = build_enum_formats(s, &pb, params, 4);
        if (cnt > 0) {
            pw_stream_update_params(s->stream, params, (uint32_t)cnt);
        }
    }
    pw_thread_loop_unlock(s->loop);
}

void psm_pw_set_fps(PsmPwStream *s, uint32_t target_fps) {
    if (!s || !s->loop || !s->stream) {
        return;
    }
    pw_thread_loop_lock(s->loop);
    s->target_fps = target_fps;
    uint8_t pbuf[16384];
    struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(pbuf, sizeof(pbuf));
    const struct spa_pod *params[16];
    size_t cnt = build_enum_formats(s, &pb, params, 16);
    if (cnt > 0) {
        pw_stream_update_params(s->stream, params, (uint32_t)cnt);
    }
    pw_thread_loop_unlock(s->loop);
}

bool psm_pw_has_error(PsmPwStream *s, char *err_buf, size_t err_buf_len) {
    if (!s) {
        return false;
    }
    pthread_mutex_lock(&s->mtx);
    bool err = s->has_error;
    if (err && err_buf && err_buf_len > 0) {
        snprintf(err_buf, err_buf_len, "%s", s->error_msg);
    }
    pthread_mutex_unlock(&s->mtx);
    return err;
}

void psm_pw_stop(PsmPwStream *s) {
    if (!s) {
        return;
    }
    if (s->loop) {
        pw_thread_loop_stop(s->loop);
        pw_thread_loop_lock(s->loop);
        if (s->stream) {
            pw_stream_disconnect(s->stream);
            pw_stream_destroy(s->stream);
            s->stream = NULL;
        }
        if (s->core) {
            pw_core_disconnect(s->core);
            s->core = NULL;
        }
        if (s->context) {
            pw_context_destroy(s->context);
            s->context = NULL;
        }
        pw_thread_loop_unlock(s->loop);
        pw_thread_loop_destroy(s->loop);
        s->loop = NULL;
    }

    pthread_mutex_lock(&s->mtx);
    close_dma_slot(&s->pending_dma);
    for (int i = 0; i < 3; i++) {
        free(s->shm_slots[i].data);
        s->shm_slots[i].data = NULL;
    }
    pthread_mutex_unlock(&s->mtx);
    pthread_mutex_destroy(&s->mtx);
    free(s);
}
