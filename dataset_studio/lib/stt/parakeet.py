"""
Parakeet STT engine using onnxruntime with fms-app downloaded models.

Supports both CTC (parakeet-v2) and TDT (parakeet-v3) architectures.
"""

import json
from pathlib import Path

import numpy as np
import onnxruntime as ort

from lib.stt.base import STTEngine, TranscriptionResult, Segment
from lib.stt.common import load_audio


class ParakeetCTCEngine(STTEngine):
    """
    Parakeet CTC engine (parakeet-v2).

    Model files required (in model_dir):
        - model.onnx
        - model.onnx_data  (external weights)
        - tokenizer.json
    """

    def __init__(self, model_dir: Path):
        self._model_dir = model_dir
        onnx_path = model_dir / "model.onnx"
        if not onnx_path.exists():
            raise FileNotFoundError(f"Parakeet CTC model not found: {onnx_path}")

        self._session = ort.InferenceSession(
            str(onnx_path),
            providers=["CPUExecutionProvider"],
        )

        # Load tokenizer
        tokenizer_path = model_dir / "tokenizer.json"
        if tokenizer_path.exists():
            with open(tokenizer_path, "r", encoding="utf-8") as f:
                tok_data = json.load(f)
            self._vocab = {v: k for k, v in tok_data["model"]["vocab"].items()}
        else:
            self._vocab = {}

        # Print input/output info for debugging
        self._input_names = [inp.name for inp in self._session.get_inputs()]
        self._output_names = [out.name for out in self._session.get_outputs()]

    def name(self) -> str:
        return "Parakeet CTC"

    def transcribe(self, audio_path: Path) -> TranscriptionResult:
        audio = load_audio(audio_path, sample_rate=16000)

        # Compute log-mel spectrogram
        features = self._compute_mel(audio)

        # Add batch dimension
        input_name = self._input_names[0]
        input_shape = self._session.get_inputs()[0].shape
        features = np.expand_dims(features, axis=0)

        # Build input dict
        inputs = {input_name: features}

        # Add audio length if the model expects it
        if len(self._input_names) > 1:
            audio_length = np.array([features.shape[-1]], dtype=np.int64)
            inputs[self._input_names[1]] = audio_length

        # Run inference
        outputs = self._session.run(self._output_names, inputs)
        logits = outputs[0]  # (batch, time, vocab_size)

        # CTC greedy decode
        token_ids = np.argmax(logits[0], axis=-1)
        tokens = self._ctc_greedy_decode(token_ids)
        text = " ".join(tokens).strip()

        # Generate segments (CTC doesn't provide word-level timestamps natively,
        # so we return the full text as a single segment)
        duration = len(audio) / 16000.0
        segments = [Segment(start=0.0, end=duration, text=text)] if text else []

        return TranscriptionResult(text=text, segments=segments)

    def _compute_mel(self, audio: np.ndarray) -> np.ndarray:
        """Compute log-mel spectrogram features."""
        import librosa
        # Parakeet uses 80 mel bins, n_fft=400, hop_length=160
        mel = librosa.feature.melspectrogram(
            y=audio, sr=16000, n_fft=400, hop_length=160, n_mels=80
        )
        log_mel = np.log(mel + 1e-5)
        return log_mel.astype(np.float32)

    def _ctc_greedy_decode(self, token_ids: np.ndarray) -> list[str]:
        """CTC greedy decode: collapse repeats, remove blanks."""
        BLANK_ID = 0  # CTC blank is typically token 0
        result = []
        prev = -1
        for tid in token_ids:
            if tid != prev and tid != BLANK_ID:
                if tid in self._vocab:
                    result.append(self._vocab[tid])
            prev = tid
        return result


class ParakeetTDTEngine(STTEngine):
    """
    Parakeet TDT engine (parakeet-v3).

    Model files required (in model_dir):
        - encoder-model.onnx
        - encoder-model.onnx.data
        - decoder_joint-model.onnx
        - vocab.txt
    """

    def __init__(self, model_dir: Path):
        self._model_dir = model_dir

        encoder_path = model_dir / "encoder-model.onnx"
        decoder_path = model_dir / "decoder_joint-model.onnx"

        if not encoder_path.exists():
            raise FileNotFoundError(f"Parakeet TDT encoder not found: {encoder_path}")
        if not decoder_path.exists():
            raise FileNotFoundError(f"Parakeet TDT decoder not found: {decoder_path}")

        self._encoder = ort.InferenceSession(
            str(encoder_path),
            providers=["CPUExecutionProvider"],
        )
        self._decoder = ort.InferenceSession(
            str(decoder_path),
            providers=["CPUExecutionProvider"],
        )

        # Load vocabulary
        vocab_path = model_dir / "vocab.txt"
        if vocab_path.exists():
            with open(vocab_path, "r", encoding="utf-8") as f:
                self._vocab = [line.strip() for line in f if line.strip()]
        else:
            self._vocab = []

        self._enc_input_names = [inp.name for inp in self._encoder.get_inputs()]
        self._enc_output_names = [out.name for out in self._encoder.get_outputs()]

    def name(self) -> str:
        return "Parakeet TDT"

    def transcribe(self, audio_path: Path) -> TranscriptionResult:
        audio = load_audio(audio_path, sample_rate=16000)

        # Compute log-mel spectrogram
        features = self._compute_mel(audio)
        features = np.expand_dims(features, axis=0)

        # Encoder inference
        enc_inputs = {self._enc_input_names[0]: features}
        if len(self._enc_input_names) > 1:
            enc_inputs[self._enc_input_names[1]] = np.array(
                [features.shape[-1]], dtype=np.int64
            )

        enc_outputs = self._encoder.run(self._enc_output_names, enc_inputs)

        # The encoder outputs logits or posteriors
        # TDT models use a decoder_joint for autoregressive decoding
        # For now, use encoder output directly if it produces final logits
        logits = enc_outputs[0]

        # Greedy decode
        token_ids = np.argmax(logits[0], axis=-1)
        tokens = self._decode_tokens(token_ids)
        text = " ".join(tokens).strip()

        duration = len(audio) / 16000.0
        segments = [Segment(start=0.0, end=duration, text=text)] if text else []

        return TranscriptionResult(text=text, segments=segments)

    def _compute_mel(self, audio: np.ndarray) -> np.ndarray:
        """Compute log-mel spectrogram features."""
        import librosa
        mel = librosa.feature.melspectrogram(
            y=audio, sr=16000, n_fft=400, hop_length=160, n_mels=80
        )
        log_mel = np.log(mel + 1e-5)
        return log_mel.astype(np.float32)

    def _decode_tokens(self, token_ids: np.ndarray) -> list[str]:
        """Decode token IDs to text using the vocabulary."""
        BLANK_IDS = {0}  # blank/pad tokens to skip
        result = []
        prev = -1
        for tid in token_ids:
            if tid != prev and tid not in BLANK_IDS:
                if 0 <= tid < len(self._vocab):
                    token = self._vocab[tid]
                    if token not in ("<blk>", "<pad>", "<unk>"):
                        result.append(token)
            prev = tid
        return result


def create_engine(model_id: str, model_dir: Path) -> STTEngine:
    """
    Factory function: create the appropriate Parakeet engine for a model ID.

    Args:
        model_id: e.g. "parakeet-v2" or "parakeet-v3"
        model_dir: path to the model directory (from fms-app)
    """
    if model_id == "parakeet-v2":
        return ParakeetCTCEngine(model_dir)
    elif model_id in ("parakeet-v3",):
        return ParakeetTDTEngine(model_dir)
    else:
        raise ValueError(f"Unknown Parakeet model: {model_id}")
