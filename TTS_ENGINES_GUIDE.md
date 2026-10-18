# MANGA-Vox Enhanced TTS Engines — Best Free Hindi/English Voices

**MANGA-Vox now includes 5 state-of-the-art open-source TTS engines** with beautiful natural Hindi audio comparable to ElevenLabs!

## 🎯 Quick Start

1. **Open Audio tab** → Click "Download AI Voice Models"
2. **Choose a Hindi voice** from the list:
   - **🌟 F5-TTS** (Recommended) — Most natural, ElevenLabs-quality
   - **Parler-TTS** (Fast) — High quality, no reference audio needed
   - XTTS v2 — Voice cloning
   - Kokoro-82M — Efficient
   - Piper — Standard reliable

## 📊 TTS Engine Comparison

| Engine | Quality | Speed | Size | Best For | Hindi Voices |
|--------|---------|-------|------|----------|-------------|
| **F5-TTS** | ⭐⭐⭐⭐⭐ | Medium | ~1.5GB | Natural storytelling | 4 + 4 English |
| **Parler-TTS** | ⭐⭐⭐⭐ | Fast | ~1GB | Quick generation | 4 + 4 English |
| **XTTS v2** | ⭐⭐⭐⭐ | Slow | ~2GB | Voice cloning | 4 + 6 English |
| **Kokoro-82M** | ⭐⭐⭐ | Fast | ~310MB | Lightweight | 4 + 11 English |
| **Piper** | ⭐⭐⭐ | Fast | Variable | Reliable fallback | 3 + 4 English |

## 🎤 Hindi Voices by Engine

### F5-TTS — State-of-the-Art (⭐ Recommended)
**Most natural sounding, latest research, ElevenLabs-class quality**
- `f5_hi_female` — Natural, warm female voice
- `f5_hi_male` — Clear, professional male voice
- `f5_hi_storyteller` — Expressive storyteller voice
- `f5_hi_narrator` — Deep, authoritative narrator

### Parler-TTS — Fast & Controllable
**Great quality, instant generation, no reference audio needed**
- `parler_hi_female_clear` — Young, bright female
- `parler_hi_female_warm` — Mature, warm female
- `parler_hi_male_deep` — Deep, professional male
- `parler_hi_male_bright` — Young, energetic male

### XTTS v2 — Voice Cloning
**ElevenLabs-level quality with voice cloning capability**
- `xtts_hi_female` — Natural female
- `xtts_hi_male` — Natural male
- `xtts_hi_storyteller` — Expressive storyteller
- `xtts_hi_narrator` — Professional narrator

### Kokoro-82M — Efficient
**Lightweight, fast, good for real-time**
- `hf_alpha` — Female alpha
- `hf_beta` — Female beta
- `hm_omega` — Male omega
- `hm_psi` — Male psi

### Piper — Reliable Fallback
- `hi_piper_f` — Female
- `hi_piper_m` — Male
- `hi_piper_rohan` — Male variant

## 🎵 English Voices

Each engine has English voices too:

### F5-TTS English
- `f5_en_female`, `f5_en_male`, `f5_en_narrator`, `f5_en_storyteller`

### Parler-TTS English
- `parler_en_female_neutral`, `parler_en_female_warm`, `parler_en_male_narrator`, `parler_en_male_casual`

### XTTS English
- `xtts_en_female`, `xtts_en_male`, `xtts_en_narrator`, `xtts_en_storyteller`, `xtts_en_warm`, `xtts_en_clear`

### Kokoro English (US + UK)
- American: `af_heart`, `af_bella`, `af_nova`, `af_sky`, `af_nicole`, `af_jessica`, `am_michael`, `am_adam`, `am_echo`, `am_eric`, `am_liam`
- British: `bf_emma`, `bf_isabella`, `bm_george`

### Piper English
- `en_piper_f`, `en_piper_m`, `en_piper_f2`, `en_piper_m2`

## 📥 Installation Guide

### Option 1: Automatic (Recommended)
1. Click "Download AI Voice Models" button in Audio tab
2. Select which engines to install
3. Wait for completion (~5-10 mins per engine, one-time)

### Option 2: Manual Python Installation
```bash
# Install F5-TTS (State-of-the-art)
pip install f5-tts torch torchaudio

# Install Parler-TTS (Fast)
pip install parler-tts transformers torch torchaudio

# Install XTTS (Voice cloning)
pip install coqui-tts torch torchaudio

# Install Kokoro (Efficient)
pip install kokoro-onnx soundfile

# Install Piper (Fallback)
pip install piper-tts onnxruntime
```

## 🚀 Which Engine Should I Use?

### I want the BEST quality Hindi ✨
→ **F5-TTS** (newest, most natural, ElevenLabs-level)

### I want it FAST 🏃
→ **Parler-TTS** (instant generation, still excellent quality)

### I want voice cloning 🎙️
→ **XTTS v2** (provide reference audio)

### I want lightweight offline 💾
→ **Kokoro-82M** (310MB, efficient)

### I want reliable fallback 🔄
→ **Piper** (always available)

## ⚙️ System Requirements

- **RAM**: 4GB minimum (8GB recommended for F5-TTS/Parler-TTS)
- **Storage**: 3-5GB for models (you can choose which engines)
- **Internet**: One-time download at installation
- **Python**: 3.10+ (3.12 recommended)

## 🔧 Troubleshooting

### "Model not found" error
→ Click "Download AI Voice Models" and wait for completion

### Slow generation
→ Try Parler-TTS (faster) or Kokoro-82M (lightweight)

### Memory error
→ Use Kokoro-82M or Piper (smaller models)

### Poor audio quality
→ Try F5-TTS or Parler-TTS (newest models)

## 💡 Pro Tips

1. **Batch your text** — Generate whole chapters at once
2. **Adjust speed** — Most voices support 0.5x to 2.0x speed
3. **Mix engines** — Use F5-TTS for important scenes, Parler-TTS for quick drafts
4. **Preload models** — First generation is slower (models loading), subsequent are faster

## 📚 Technical Details

### F5-TTS
- **Research**: ByteDance
- **Multilingual**: Yes (20+ languages)
- **Architecture**: Flow-based TTS
- **Quality**: State-of-the-art (research grade)
- **Size**: ~1.5 GB

### Parler-TTS
- **Research**: OpenAI-inspired
- **Multilingual**: Yes
- **Architecture**: Transformer-based
- **Quality**: Production-grade
- **Size**: ~1 GB

### XTTS v2
- **Research**: Coqui TTS
- **Multilingual**: Yes
- **Architecture**: Tacotron2-based with multilingual support
- **Features**: Voice cloning via reference audio
- **Size**: ~2 GB

## 🎓 Learning More

- [F5-TTS GitHub](https://github.com/svjack/F5-TTS)
- [Parler-TTS GitHub](https://github.com/microsoft/parler_tts)
- [XTTS v2 Docs](https://docs.coqui.ai/en/latest/inference.html)
- [Kokoro-ONNX](https://github.com/thewh1teagle/kokoro-onnx)
- [Piper TTS](https://github.com/rhasspy/piper)

---

**Your audio, your way — fully offline, no APIs, premium quality!** 🎉
