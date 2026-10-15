// Shared Realtime WebSocket to /v1/realtime. One connection is shared between
// the "WebSocket test" and "Wake word" cards (they can't run at the same time
// anyway — one microphone), exactly like the old single-script page.

import type { RealtimeMessage } from '../types';

type MessageListener = (msg: RealtimeMessage) => void;
type CloseListener = (ev: CloseEvent) => void;

export class RealtimeClient {
  private ws: WebSocket | null = null;
  private messageListeners = new Set<MessageListener>();
  private closeListeners = new Set<CloseListener>();

  /** Server session id, captured from session.created. */
  sessionId = '';

  get url(): string {
    return (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/v1/realtime';
  }

  get isOpen(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  onMessage(cb: MessageListener): () => void {
    this.messageListeners.add(cb);
    return () => this.messageListeners.delete(cb);
  }

  onClose(cb: CloseListener): () => void {
    this.closeListeners.add(cb);
    return () => this.closeListeners.delete(cb);
  }

  /** Connect, reusing an already-open (or connecting) socket. */
  connect(): Promise<void> {
    if (this.isOpen) return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      if (this.ws && this.ws.readyState === WebSocket.CONNECTING) {
        this.ws.addEventListener('open', () => resolve(), { once: true });
        this.ws.addEventListener('error', () => reject(new Error('websocket error')), { once: true });
        return;
      }
      const ws = new WebSocket(this.url);
      this.ws = ws;
      ws.onmessage = (ev: MessageEvent) => {
        let msg: RealtimeMessage;
        try {
          msg = JSON.parse(ev.data as string);
        } catch {
          return;
        }
        if (msg.type === 'session.created') {
          this.sessionId = (msg.session && msg.session.id) || '';
        }
        for (const cb of [...this.messageListeners]) cb(msg);
      };
      ws.onclose = (ev: CloseEvent) => {
        if (this.ws !== ws) return;
        this.ws = null;
        for (const cb of [...this.closeListeners]) cb(ev);
      };
      ws.addEventListener('open', () => resolve(), { once: true });
      ws.addEventListener('error', () => reject(new Error('websocket error')), { once: true });
    });
  }

  send(obj: unknown): void {
    if (this.isOpen) this.ws!.send(JSON.stringify(obj));
  }
}

export const realtime = new RealtimeClient();
