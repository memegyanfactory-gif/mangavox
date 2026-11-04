import unittest

from python import video_generator as vg


GPU = ["-c:v", "h264_nvenc"]
CPU = ["-c:v", "libx264"]


class EncoderFallbackTests(unittest.TestCase):
    def setUp(self):
        self.saved = vg._NVENC
        vg._NVENC = True

    def tearDown(self):
        vg._NVENC = self.saved

    def test_nvenc_is_dropped_only_when_the_cpu_then_works(self):
        tried = []
        ok, _ = vg.run_codecs([GPU, CPU], lambda c: (tried.append(c[1]) or c is CPU, "boom"))
        self.assertTrue(ok)
        self.assertEqual(tried, ["h264_nvenc", "libx264"])
        self.assertFalse(vg._NVENC)

    def test_a_run_broken_on_both_encoders_keeps_nvenc(self):
        ok, err = vg.run_codecs([GPU, CPU], lambda c: (False, f"{c[1]} failed"))
        self.assertFalse(ok)
        self.assertEqual(err, "libx264 failed")
        self.assertTrue(vg._NVENC)

    def test_the_first_encoder_that_works_is_the_only_one_run(self):
        tried = []
        ok, _ = vg.run_codecs([GPU, CPU], lambda c: (tried.append(c[1]) or True, ""))
        self.assertTrue(ok and tried == ["h264_nvenc"] and vg._NVENC)


class ClipReuseTests(unittest.TestCase):
    def test_a_clip_left_broken_by_a_power_cut_is_not_reused(self):
        import os, subprocess, tempfile
        with tempfile.TemporaryDirectory() as d:
            self.assertFalse(vg.clip_usable(os.path.join(d, "missing.mp4"), 2))
            broken = os.path.join(d, "broken.mp4")
            with open(broken, "wb") as f:
                f.write(b"\0" * 4096)  # the right size, nothing in it
            self.assertFalse(vg.clip_usable(broken, 2))
            good = os.path.join(d, "good.mp4")
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "color=black:s=64x64:r=30:d=0.1",
                            "-frames:v", "3", "-c:v", "libx264", "-pix_fmt", "yuv420p", good], check=True)
            self.assertTrue(vg.clip_usable(good, 3))
            self.assertFalse(vg.clip_usable(good, 4), "cut short")


class WarpTests(unittest.TestCase):
    def test_a_camera_that_does_not_move_is_worked_out_once(self):
        still = vg._warp(("0.5", "0.5"), "1.08", 90, "cubic")
        self.assertIn("eval=init", still)

    def test_a_moving_camera_is_worked_out_every_frame(self):
        for zoom, window in (("1+0.1*{e}", ("0.5", "0.5")), ("1.08", ("{e}", "0.5")),
                             ("1+0.12*pow(1-min(1,on/7),3)", ("0.5", "0.5"))):
            self.assertIn("eval=frame", vg._warp(window, zoom, 90, "cubic"), (zoom, window))


if __name__ == "__main__":
    unittest.main()
