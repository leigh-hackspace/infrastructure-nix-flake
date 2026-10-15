// Shape of GET /status.json (see the Rust StatusStore::snapshot in src/status.rs).

export interface CurrentItem {
  item_id: string;
  elapsed_secs: number;
  audio_secs: number;
}

export interface SessionInfo {
  id: string;
  connected_secs: number;
  last_event_secs_ago: number;
  buffered_secs: number;
  current_item: CurrentItem | null;
}

export interface RecentItem {
  item_id: string;
  session_id: string;
  completed_secs_ago: number;
  audio_secs: number;
  duration_ms: number;
  language: string | null;
  transcript: string;
  ok: boolean;
  error: string | null;
  has_audio: boolean;
}

export interface WhisperHealth {
  ok: boolean;
  latency_ms: number;
  checked_secs_ago: number;
  error: string | null;
}

export interface StatusSnapshot {
  uptime_secs: number;
  whisper_url: string;
  whisper: WhisperHealth;
  connections: number;
  sessions_total: number;
  transcriptions_total: number;
  failed_total: number;
  sessions: SessionInfo[];
  recent: RecentItem[];
}

// Events the gateway sends over /v1/realtime (see src/realtime.rs).
export interface RealtimeMessage {
  type: string;
  session?: { id?: string };
  item_id?: string;
  transcript?: string;
  error?: { message?: string };
}
