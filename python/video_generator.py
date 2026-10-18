"""
Video Generator for MangaVox
Combines manga page images + TTS audio into a narrated video with transitions.
Requires: ffmpeg (brew install ffmpeg)
"""
import os
import sys
import json
import subprocess
import shutil
import wave
import tempfile
import math

def check_ffmpeg():
    """Verify ffmpeg is available."""
    if shutil.which("ffmpeg") is None:
        raise RuntimeError(
            "ffmpeg not found. Install it with: brew install ffmpeg"
        )

def get_audio_duration_sec(audio_path):
    """Get audio duration in seconds using ffprobe, falling back to WAV headers."""
    try:
        result = subprocess.run(
            [
                "ffprobe", "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                audio_path,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            duration = float((result.stdout or "").strip())
            if math.isfinite(duration) and duration > 0:
                return duration
    except Exception:
        pass
    try:
        with wave.open(audio_path, 'r') as wf:
            return wf.getnframes() / wf.getframerate()
    except Exception:
        return 5.0

def get_resolution(preset):
    """Return (width, height) for a resolution preset."""
    presets = {
        "720p":  (1280, 720),
        "1080p": (1920, 1080),
        "1440p": (2560, 1440),
        "4k":    (3840, 2160),
        "2160p": (3840, 2160),
    }
    return presets.get(preset, (1920, 1080))


ZOOMPAN_PRESETS = [
    "zoompan=z='min(zoom+0.0003\\,1.08)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'",
    "zoompan=z='max(1.08-0.0003*on\\,1.0)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'",
    "zoompan=z='1.06':x='(iw-iw/zoom)*(on/d)':y='ih/2-(ih/zoom/2)'",
    "zoompan=z='1.06':x='(iw-iw/zoom)*(1-on/d)':y='ih/2-(ih/zoom/2)'",
    "zoompan=z='1.06':x='iw/2-(iw/zoom/2)':y='(ih-ih/zoom)*(on/d)'",
    "zoompan=z='1.06':x='iw/2-(iw/zoom/2)':y='(ih-ih/zoom)*(1-on/d)'",
    "zoompan=z='1.08':x='(iw-iw/zoom)*(on/d)':y='(ih-ih/zoom)*(on/d)'",
    "zoompan=z='1.08':x='(iw-iw/zoom)*(1-on/d)':y='(ih-ih/zoom)*(1-on/d)'",
    "zoompan=z='1.04+0.04*sin(2*3.14159*on/90)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'",
    "zoompan=z='1.0+min(0.08\\,0.08*(on/15))':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'"
]

def combine_audio_files(audio_paths, output_path):
    """Concatenate WAV files into a single WAV using ffmpeg."""
    if os.path.exists(output_path):
        return

    list_file = output_path + ".txt"
    with open(list_file, 'w') as f:
        for p in audio_paths:
            f.write(f"file '{p}'\n")

    subprocess.run([
        "ffmpeg", "-y", "-f", "concat", "-safe", "0",
        "-i", list_file,
        "-c:a", "pcm_s16le", "-ar", "22050", "-ac", "1",
        output_path
    ], capture_output=True, timeout=300)

    os.unlink(list_file)

def generate_video(page_files, sync_map, output_dir, config=None, progress_cb=None):
    """
    Generate a narrated manga video.

    Args:
        page_files: sorted list of page image paths
        sync_map: dict with 'segments' (page→timing) from sync_map.json
        output_dir: pipeline output directory
        config: dict with options:
            - transition: 'fade' | 'crossfade' | 'none' (default: 'fade')
            - resolution: '720p' | '1080p' | '1440p' (default: '1080p')
            - transition_duration: float seconds (default: 0.5)
            - effect: 'ken_burns' | 'none' (default: 'ken_burns')
        progress_cb: function(stage, current, total)

    Returns:
        path to final video file
    """
    check_ffmpeg()
    config = config or {}

    transition = config.get("transition", "fade")
    resolution = config.get("resolution", "1080p")
    trans_dur = config.get("transition_duration", 0.5)
    effect = config.get("effect", "ken_burns")
    if transition == "cut":
        transition = "none"
    if transition == "zoom-punch":
        transition = "fade"
        effect = "ken_burns"
    if transition == "crossfade":
        # Narrated manga must keep page starts exactly aligned to audio starts.
        # Real crossfades overlap clips and shorten the timeline, so treat this
        # as a per-page visual fade instead of overlapping audio/video.
        transition = "fade"
    force_rerender = config.get("force_rerender", False)
    w, h = get_resolution(resolution)

    video_dir = os.path.join(output_dir, "video_parts")
    os.makedirs(video_dir, exist_ok=True)

    # Prefer panel-level beats (each panel image synced to its slice of the page
    # audio) so the video advances panel-by-panel with the narration. Fall back
    # to page-level segments when no beats were produced.
    beat_segments = sync_map.get("beat_segments") or []
    segments = sync_map.get("segments", [])
    use_beats = bool(beat_segments)
    render_items = beat_segments if use_beats else segments
    if not render_items:
        raise RuntimeError("No segments in sync map — nothing to render")

    audio_dir = os.path.join(output_dir, "audio")
    final_video = os.path.join(output_dir, "video_final.mp4")

    # Allow re-render when user explicitly requests it
    if force_rerender:
        if os.path.exists(final_video):
            try:
                os.unlink(final_video)
            except OSError:
                pass
        if os.path.isdir(video_dir):
            try:
                shutil.rmtree(video_dir)
                os.makedirs(video_dir, exist_ok=True)
            except OSError:
                pass
    elif os.path.exists(final_video) and os.path.getsize(final_video) > 1000:
        return final_video

    # ── Step 1: Create per-panel (or per-page) video segments ──
    segment_paths = []
    total = len(render_items)

    for idx, seg in enumerate(render_items):
        if progress_cb:
            progress_cb("video", idx + 1, total * 3)  # 3 phases: segments, concat, finalise

        page_num = seg["page"]
        audio_file = seg.get("audio_file", f"page_{page_num:04d}.wav")
        audio_path = os.path.join(audio_dir, audio_file)

        if not os.path.exists(audio_path):
            continue

        # Image: a beat carries its exact panel crop; otherwise resolve the page.
        page_image = seg.get("panel_image") if use_beats else None
        if page_image and not os.path.exists(page_image):
            page_image = None
        if not page_image:
            for pf in page_files:
                basename = os.path.basename(pf)
                # Match page number (0001.jpg, page_0001.jpg, etc.)
                if f"{page_num:04d}" in basename or f"{page_num + 1:04d}" in basename:
                    page_image = pf
                    break
        if not page_image and isinstance(page_num, int) and page_num < len(page_files):
            page_image = page_files[page_num]
        if not page_image:
            continue

        # Crop to focus point if present (aspect ratio matched to eliminate black margins)
        focus_point = seg.get("focus_point")
        if focus_point and len(focus_point) == 4:
            try:
                from PIL import Image
                img = Image.open(page_image)
                img_w, img_h = img.size
                
                ymin, xmin, ymax, xmax = focus_point
                y_min = int(ymin * img_h / 1000)
                x_min = int(xmin * img_w / 1000)
                y_max = int(ymax * img_h / 1000)
                x_max = int(xmax * img_w / 1000)
                
                # Apply 15% margin padding
                box_w = x_max - x_min
                box_h = y_max - y_min
                pad_w = int(box_w * 0.15)
                pad_h = int(box_h * 0.15)
                
                x_min = max(0, x_min - pad_w)
                y_min = max(0, y_min - pad_h)
                x_max = min(img_w, x_max + pad_w)
                y_max = min(img_h, y_max + pad_h)
                
                # Adjust coordinates to match target aspect ratio (w/h)
                box_w = x_max - x_min
                box_h = y_max - y_min
                if box_w > 0 and box_h > 0:
                    target_aspect = w / h
                    box_aspect = box_w / box_h
                    if box_aspect > target_aspect:
                        # Expand height
                        new_h = int(box_w / target_aspect)
                        center_y = (y_min + y_max) // 2
                        y_min = max(0, center_y - new_h // 2)
                        y_max = min(img_h, y_min + new_h)
                        if y_max - y_min < new_h:
                            y_min = max(0, y_max - new_h)
                    else:
                        # Expand width
                        new_w = int(box_h * target_aspect)
                        center_x = (x_min + x_max) // 2
                        x_min = max(0, center_x - new_w // 2)
                        x_max = min(img_w, x_min + new_w)
                        if x_max - x_min < new_w:
                            x_min = max(0, x_max - new_w)
                    
                    cropped_img = img.crop((x_min, y_min, x_max, y_max))
                    temp_crop_path = os.path.join(video_dir, f"crop_{idx:04d}.jpg")
                    cropped_img.save(temp_crop_path, "JPEG", quality=95)
                    page_image = temp_crop_path
            except Exception as e:
                print(f"[video] Failed to crop page to focus point: {e}", file=sys.stderr)

        seg_path = os.path.join(video_dir, f"seg_{idx:04d}.mp4")

        # Skip if already rendered
        if os.path.exists(seg_path) and os.path.getsize(seg_path) > 100:
            segment_paths.append(seg_path)
            continue

        # For beats, play just this panel's slice of the page audio; for pages,
        # use the whole file.
        audio_offset = 0.0
        if use_beats and seg.get("duration_ms"):
            audio_offset = max(0.0, float(seg.get("audio_offset_ms", 0)) / 1000.0)
            duration = float(seg["duration_ms"]) / 1000.0
        else:
            duration = get_audio_duration_sec(audio_path)
        if duration < 0.1:
            continue
        duration = round(duration, 3)

        # Build video filter for this segment
        vf_parts = [
            # Scale + pad to target resolution, preserving aspect ratio
            f"scale={w}:{h}:force_original_aspect_ratio=decrease",
            f"pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black",
            "setsar=1",
        ]

        if effect != "none":
            # Cycle through ZOOMPAN_PRESETS based on segment index
            preset_idx = idx % len(ZOOMPAN_PRESETS)
            preset_str = ZOOMPAN_PRESETS[preset_idx]
            frames = max(1, int(duration * 30))
            zoompan_expr = f"{preset_str}:d={frames}:s={w}x{h}:fps=30"
            vf_parts = [
                f"scale={int(w*1.2)}:{int(h*1.2)}:force_original_aspect_ratio=decrease",
                f"pad={int(w*1.2)}:{int(h*1.2)}:(ow-iw)/2:(oh-ih)/2:color=black",
                zoompan_expr
            ]

        if transition == "fade":
            # Add fade in/out
            frames = int(duration * 30)
            fade_frames = min(int(trans_dur * 30), frames // 4)
            vf_parts.append(f"fade=in:0:{fade_frames}")
            vf_parts.append(f"fade=out:{max(0, frames - fade_frames)}:{fade_frames}")

        vf = ",".join(vf_parts)

        # When rendering a beat, seek into the page audio so this clip carries
        # only the panel's slice (-ss before -i seeks the audio input).
        audio_in = ["-i", audio_path]
        if audio_offset > 0.001:
            audio_in = ["-ss", f"{audio_offset:.3f}", "-i", audio_path]

        cmd = [
            "ffmpeg", "-y",
            "-hide_banner", "-loglevel", "error",
            "-loop", "1", "-i", page_image,
            *audio_in,
            "-vf", vf,
            "-map", "0:v:0", "-map", "1:a:0",
            "-c:v", "libx264", "-preset", "fast", "-crf", "23",
            "-c:a", "aac", "-b:a", "128k",
            "-af", "aresample=async=1:first_pts=0",
            "-r", "30",
            "-pix_fmt", "yuv420p",
            "-movflags", "+faststart",
            "-t", f"{duration:.3f}",
            seg_path,
        ]

        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
            if result.returncode != 0:
                print(f"[video] FFmpeg error for page {page_num}: {result.stderr[:300]}", file=sys.stderr)
                continue
            segment_paths.append(seg_path)
        except subprocess.TimeoutExpired:
            print(f"[video] Timeout rendering page {page_num}", file=sys.stderr)
            continue

    if not segment_paths:
        raise RuntimeError("No video segments were generated")

    # ── Step 2: Concatenate all segments ──
    if progress_cb:
        progress_cb("video", total + 1, total * 3)

    concat_list = os.path.join(video_dir, "concat.txt")
    with open(concat_list, 'w') as f:
        for sp in segment_paths:
            f.write(f"file '{sp}'\n")

    concat_video = os.path.join(video_dir, "concat.mp4")

    _simple_concat(concat_list, concat_video)

    if not os.path.exists(concat_video):
        raise RuntimeError("Failed to concatenate video segments")

    # ── Step 3: Burn captions + mix background music + move to final location ──
    if progress_cb:
        progress_cb("video", total * 3, total * 3)

    bg_music = config.get("bg_music")
    bg_music_volume = float(config.get("bg_music_volume") or 0.15)
    narration_volume = float(config.get("narration_volume") or 1.0)
    bg_music_loop = bool(config.get("bg_music_loop", True))
    
    subtitle_theme = config.get("subtitle_theme", "Classic White")
    subtitle_anim = config.get("subtitle_anim", "Karaoke Sweep")
    want_captions = config.get("captions", True)

    ass_path = None
    if want_captions:
        try:
            import captions
            ass_path = captions.write_ass_captions(
                render_items,
                output_dir,
                theme_name=subtitle_theme,
                anim_style=subtitle_anim,
                filename="captions.ass"
            )
        except Exception as e:
            print(f"[video] Failed to generate ASS captions: {e}", file=sys.stderr)

    success = _finalize_video_with_audio_mix_and_captions(
        concat_video,
        final_video,
        ass_path,
        bg_music,
        narration_volume,
        bg_music_volume,
        bg_music_loop,
        w,
        h
    )

    if success:
        try:
            os.unlink(concat_video)
        except OSError:
            pass
    else:
        print("[video] Complex finalization failed, falling back to simple video copy", file=sys.stderr)
        try:
            if os.path.exists(final_video):
                os.unlink(final_video)
            shutil.move(concat_video, final_video)
        except Exception as e:
            print(f"[video] Fallback failed: {e}", file=sys.stderr)

    # Clean up temp segment files
    try:
        shutil.rmtree(video_dir)
    except Exception:
        pass

    return final_video


def _finalize_video_with_audio_mix_and_captions(
    input_video, output_video, ass_path, bg_music,
    narration_vol, bg_vol, bg_loop, w, h
):
    """
    Adjusts narration volume, mixes background music, loops it, and burns subtitles
    in a single FFmpeg command.
    """
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
    
    # Input 0: Concatenated video
    cmd += ["-i", input_video]
    
    # Input 1: Background music (if present)
    has_bg = bg_music and os.path.exists(bg_music)
    if has_bg:
        if bg_loop:
            cmd += ["-stream_loop", "-1"]
        cmd += ["-i", bg_music]
        
    filter_complex = []
    
    # 1. Audio processing
    if has_bg:
        filter_complex.append(
            f"[0:a]volume={narration_vol}[a0];"
            f"[1:a]volume={bg_vol}[a1];"
            f"[a0][a1]amix=inputs=2:duration=first:dropout_transition=2[a]"
        )
    else:
        filter_complex.append(f"[0:a]volume={narration_vol}[a]")
        
    # 2. Subtitle processing
    has_video_filter = False
    if ass_path and os.path.exists(ass_path):
        esc = ass_path.replace("\\", "\\\\").replace(":", "\\:").replace("'", "\\'")
        filter_complex.append(f"[0:v]subtitles='{esc}'[v]")
        has_video_filter = True
        
    cmd += ["-filter_complex", ";".join(filter_complex)]
    
    if has_video_filter:
        cmd += ["-map", "[v]"]
    else:
        cmd += ["-map", "0:v"]
        
    cmd += ["-map", "[a]"]
    
    if has_video_filter:
        cmd += ["-c:v", "libx264", "-preset", "fast", "-crf", "23"]
    else:
        cmd += ["-c:v", "copy"]
        
    cmd += [
        "-c:a", "aac", "-b:a", "128k",
        "-movflags", "+faststart",
        output_video
    ]
    
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        if result.returncode != 0:
            print(f"[video] Finalization command failed: {result.stderr[:400]}", file=sys.stderr)
            return False
        return os.path.exists(output_video) and os.path.getsize(output_video) > 1000
    except Exception as e:
        print(f"[video] Finalization command error: {e}", file=sys.stderr)
        return False


def _simple_concat(concat_list, output):
    """Concatenate videos with fresh timestamps to prevent accumulated drift."""
    subprocess.run([
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-fflags", "+genpts",
        "-f", "concat", "-safe", "0",
        "-i", concat_list,
        "-c:v", "libx264", "-preset", "fast", "-crf", "23",
        "-c:a", "aac", "-b:a", "128k",
        "-af", "aresample=async=1:first_pts=0",
        "-r", "30",
        "-pix_fmt", "yuv420p",
        "-movflags", "+faststart",
        output,
    ], check=True, capture_output=True, timeout=900)


def _crossfade_concat(segment_paths, output, trans_dur, w, h):
    """Concatenate with crossfade transitions using xfade filter."""
    if len(segment_paths) < 2:
        _simple_concat_from_paths(segment_paths, output)
        return

    cmd = ["ffmpeg", "-y"]

    # Add all inputs
    for sp in segment_paths:
        cmd += ["-i", sp]

    # Build xfade filter chain
    n = len(segment_paths)
    filter_parts = []
    durations = []

    # Get durations of each segment
    for sp in segment_paths:
        try:
            result = subprocess.run(
                ["ffprobe", "-v", "quiet", "-show_entries", "format=duration",
                 "-of", "csv=p=0", sp],
                capture_output=True, text=True, timeout=30
            )
            durations.append(float(result.stdout.strip()))
        except Exception:
            durations.append(5.0)

    # Build xfade chain
    offset = durations[0] - trans_dur
    if n == 2:
        filter_parts.append(
            f"[0:v][1:v]xfade=transition=fade:duration={trans_dur}:offset={max(0, offset)}[v]"
        )
        filter_parts.append(
            f"[0:a][1:a]acrossfade=d={trans_dur}[a]"
        )
        cmd += ["-filter_complex", ";".join(filter_parts)]
        cmd += ["-map", "[v]", "-map", "[a]"]
    else:
        # Chain xfades for multiple inputs
        prev_v = "0:v"
        prev_a = "0:a"

        for i in range(1, n):
            out_v = f"v{i}" if i < n - 1 else "v"
            out_a = f"a{i}" if i < n - 1 else "a"
            offset = max(0.1, sum(durations[:i]) - trans_dur * i)

            filter_parts.append(
                f"[{prev_v}][{i}:v]xfade=transition=fade:duration={trans_dur}:offset={offset}[{out_v}]"
            )
            filter_parts.append(
                f"[{prev_a}][{i}:a]acrossfade=d={trans_dur}[{out_a}]"
            )
            prev_v = out_v
            prev_a = out_a

        cmd += ["-filter_complex", ";".join(filter_parts)]
        cmd += ["-map", "[v]", "-map", "[a]"]

    cmd += [
        "-c:v", "libx264", "-preset", "fast", "-crf", "23",
        "-c:a", "aac", "-b:a", "128k",
        "-pix_fmt", "yuv420p",
        output,
    ]

    subprocess.run(cmd, capture_output=True, timeout=1800)


def _simple_concat_from_paths(paths, output):
    """Concat from a list of paths."""
    tmp = output + ".txt"
    with open(tmp, 'w') as f:
        for p in paths:
            f.write(f"file '{p}'\n")
    _simple_concat(tmp, output)
    os.unlink(tmp)
