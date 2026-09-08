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
