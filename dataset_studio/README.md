# Dataset Studio

Python command-line tools to build and manage dictation datasets for the FMS app.

A **dataset** is a folder containing audio files plus all the derived data the app needs: subtitles, waveforms, and a SQLite database. These scripts let you build datasets on any machine — no need to run the desktop app.

## Requirements

- **Python 3.10+**
- **audiowaveform** — for waveform generation ([install guide](https://github.com/bbc/audiowaveform#installation))
- **STT model** — for subtitle generation (e.g. Whisper via `faster-whisper`)

Install Python dependencies:

```bash
pip install -r requirements.txt
```

## Quick Start

```bash
# 1. Create a new dataset
python init_dataset.py /data/my_dataset --name "My Dataset"

# 2. Add audio files
python import_media.py /data/my_dataset /path/to/audio_files/

# 3. Generate subtitles (requires STT model)
python generate_subtitles.py /data/my_dataset --model large-v3

# 4. Generate waveforms (requires audiowaveform)
python generate_waveforms.py /data/my_dataset

# 5. Build the database
python build_database.py /data/my_dataset
```

The `/data/my_dataset` folder is now ready to use with the FMS app.

## Optional Steps

```bash
# Add a reference book for cue alignment
# (place book.txt in the dataset folder first)
python parse_book.py /data/my_dataset
python align_cues.py /data/my_dataset

# Import plain-text transcripts
# (place .txt files in the transcript/ folder first)
python write_transcripts.py /data/my_dataset
```

## Command Reference

| Command | Description |
|---------|-------------|
| `init_dataset.py <path> [--name NAME]` | Create a new dataset directory with `info.json` |
| `import_media.py <path> <source>` | Copy audio files into the dataset's `media/` folder |
| `generate_subtitles.py <path> [--model MODEL]` | Transcribe audio → VTT subtitle files |
| `generate_waveforms.py <path>` | Generate waveform JSON files for the timeline view |
| `build_database.py <path>` | Build `data.sqlite3` from subtitles, media, and transcripts |
| `parse_book.py <path>` | Split `book.txt` into reference chunks |
| `align_cues.py <path>` | Match subtitle cues to reference text |
| `write_transcripts.py <path>` | Import transcript files into the database |

Every command takes the **dataset directory path** as its first argument.

## Using the Dataset with the App

1. Open the FMS app → **Settings** → set the **Datasets Directory** to the parent folder containing your dataset.
2. Go to **Datasets** → click **Refresh** — your dataset should appear in the list.
3. If you only ran stages 0–2 (subtitles + waveforms but no database), click **Build Database** on the dataset card.

## Further Reading

See [DESIGN.md](DESIGN.md) for the full technical architecture, database schema, and pipeline details.
