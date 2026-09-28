/** A voice available from Edge TTS. */
export interface TtsVoice {
  name: string;
  short_name: string;
  locale: string;
  gender: string;
}

/** Result of a TTS synthesis operation. */
export interface TtsSynthesizeResult {
  audio_len: number;
  output_path: string;
}

/** Result of a TTS preview operation (audio data as base64). */
export interface TtsPreviewResult {
  audio_base64: string;
}
