# Dataset Studio

Python command-line tools to build and manage dictation datasets for the FMS app.

A **dataset** is a folder containing audio files plus all the derived data the app needs: subtitles, waveforms, and a SQLite database. These scripts let you build datasets on any machine — no need to run the desktop app.

## Requirements

- **Miniconda3** — Python environment management
- **audiowaveform** — for waveform generation ([install guide](https://github.com/bbc/audiowaveform#installation))
- **fms-app desktop client** — download STT models via the Models tab; the Python scripts reuse them automatically

## Win 11: Miniconda3 in Powershell
```bash
1, Download and install
https://www.anaconda.com/download/success

2, Update the PowerShell Execution Policy
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser

3, Initialize Conda for PowerShell
Search and open Anaconda Prompt from the Start menu
conda init powershell

4, Verify the Setup
conda --version

5, Use mirror
https://mirrors.tuna.tsinghua.edu.cn/help/anaconda/
```

## Set up the environment:

```bash
conda env create -f environment.yml
conda activate dataset_studio
```

## Quick Start

```bash
# 1. Create a new dataset
python scripts/init_dataset.py /data/my_dataset --name "My Dataset"

# 2. Add audio files
python scripts/import_media.py /data/my_dataset /path/to/audio_files/

# 3. Generate subtitles (uses ONNX models downloaded by fms-app)
python scripts/generate_subtitles.py /data/my_dataset --model parakeet-v3

# 4. Generate waveforms (requires audiowaveform)
python scripts/generate_waveforms.py /data/my_dataset

# 5. Build the database
python scripts/build_database.py /data/my_dataset
```

The `/data/my_dataset` folder is now ready to use with the FMS app.

## Optional Steps

```bash
# Add a reference book for cue alignment
# (place book.txt in the dataset folder first)
python scripts/split_book.py /data/my_dataset
python scripts/align_cues_book.py /data/my_dataset

# Or align using per-audio transcripts (more accurate if available)
python scripts/align_cues_transcript.py /data/my_dataset

# Import plain-text transcripts
# (place .txt files in the transcript/ folder first)
python scripts/write_transcripts.py /data/my_dataset
```

## Command Reference

| Command | Description |
|---------|-------------|
| `scripts/init_dataset.py <path> [--name NAME]` | Create a new dataset directory with `info.json` |
| `scripts/import_media.py <path> <source>` | Copy audio files into the dataset's `media/` folder |
| `scripts/generate_subtitles.py <path> [--model MODEL]` | Transcribe audio → VTT subtitle files |
| `scripts/generate_waveforms.py <path>` | Generate waveform JSON files for the timeline view |
| `scripts/build_database.py <path>` | Build `data.sqlite3` from subtitles, media, and transcripts |
| `scripts/split_book.py <path>` | Split `book.txt` into sentences → `book_sentences.txt` cache |
| `scripts/align_cues_book.py <path>` | Match subtitle cues to book reference text |
| `scripts/align_cues_transcript.py <path>` | Match subtitle cues to per-audio transcript text |
| `scripts/write_transcripts.py <path>` | Import transcript files into the database |

Every command takes the **dataset directory path** as its first argument.

## Using the Dataset with the App

1. Open the FMS app → **Settings** → set the **Datasets Directory** to the parent folder containing your dataset.
2. Go to **Datasets** → click **Refresh** — your dataset should appear in the list.
3. If you only ran stages 0–2 (subtitles + waveforms but no database), click **Build Database** on the dataset card.

## Capturing Audio from the Web

Some audio sources (podcasts, streaming services, educational platforms) don't offer direct downloads. Use the **Web Audio Recorder** browser extension to capture these.

### Setup

1. Install the extension:
   - [Chrome Web Store](https://chromewebstore.google.com/detail/web-audio-recorder/mijllbegagcedcglnbpkhofabiknfgjf)
   - [Edge Add-ons](https://microsoftedge.microsoft.com/addons/detail/web-audio-recorder/ipjmdppkpjccddbnabmhjcbfiaegonbn)

2. Open the extension and configure:
   - **Compress**: ON (to get `.mp3` output)
   - **Bit Rate**: 128 kbps or higher
   - **Max Recording Duration**: set to cover the full length of your audio

### Recording

1. Navigate to the page with the audio you want to capture
2. Click the Web Audio Recorder icon to start recording
3. Play the audio in the browser
4. When playback finishes, click the extension icon again to stop and download
5. Save the file into your dataset's `media/` folder

### Tips

- **One file per track**: record each audio segment separately for cleaner subtitle generation
- **Let it play to the end**: avoid stopping early — extra silence at the end is harmless, but cutting off mid-sentence hurts subtitle quality
- **Check the format**: the extension can output `.mp3` or `.wav`; both are supported
- **For video pages**: the extension captures the audio track from `<video>` elements too

## Further Reading

See [DESIGN.md](DESIGN.md) for the full technical architecture, database schema, and pipeline details.
