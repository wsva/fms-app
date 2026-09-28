/** Configuration for the built-in web server. */
export interface WebServiceConfig {
  port: number;
  stt: boolean;
  dataset: boolean;
  tts: boolean;
}

/** Runtime status of the web server, returned by the backend. */
export interface WebServiceStatus {
  running: boolean;
  port: number;
  stt: boolean;
  dataset: boolean;
  tts: boolean;
  local_url: string | null;
  lan_url: string | null;
}
