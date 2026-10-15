// Microphone capture via AudioWorklet (createScriptProcessor is deprecated).
// The worklet accumulates 4096-sample (~85 ms @ 48 kHz) chunks and posts them
// to the main thread; the consumer resamples/encodes them as needed.

const MIC_CHUNK = 4096;

const WORKLET_SRC = `
class MicProcessor extends AudioWorkletProcessor {
  constructor() { super(); this.buf = new Float32Array(${MIC_CHUNK}); this.pos = 0; }
  process(inputs) {
    const input = inputs[0] && inputs[0][0];
    if (input) {
      for (let i = 0; i < input.length; i++) {
        this.buf[this.pos++] = input[i];
        if (this.pos === this.buf.length) {
          this.port.postMessage(this.buf);
          this.pos = 0;
        }
      }
    }
    return true;
  }
}
registerProcessor('mic-processor', MicProcessor);
`;

type AudioContextCtor = typeof AudioContext;

function audioContextCtor(): AudioContextCtor | undefined {
  return window.AudioContext || (window as unknown as { webkitAudioContext?: AudioContextCtor }).webkitAudioContext;
}

export class MicCapture {
  private ctx: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private srcNode: MediaStreamAudioSourceNode | null = null;
  private workletNode: AudioWorkletNode | null = null;
  private workletUrl: string | null = null; // blob URL for the worklet module (cached)

  get context(): AudioContext | null {
    return this.ctx;
  }

  get sampleRate(): number {
    return this.ctx?.sampleRate ?? 48000;
  }

  async start(onChunk: (f32: Float32Array) => void): Promise<void> {
    await this.stop();
    const Ctx = audioContextCtor();
    if (!Ctx) throw new Error('this browser has no Web Audio support');
    if (!navigator.mediaDevices?.getUserMedia) {
      throw new Error('microphone needs a secure origin — open https://whisper.int.leighhack.org');
    }
    this.ctx = new Ctx();
    this.stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    await this.ctx.resume();
    if (!this.workletUrl) {
      this.workletUrl = URL.createObjectURL(new Blob([WORKLET_SRC], { type: 'application/javascript' }));
    }
    await this.ctx.audioWorklet.addModule(this.workletUrl);
    this.srcNode = this.ctx.createMediaStreamSource(this.stream);
    this.workletNode = new AudioWorkletNode(this.ctx, 'mic-processor');
    this.workletNode.port.onmessage = (e: MessageEvent) => onChunk(e.data as Float32Array);
    this.srcNode.connect(this.workletNode);
    this.workletNode.connect(this.ctx.destination);
  }

  /** Stop capture and release everything (idempotent). */
  async stop(): Promise<void> {
    if (this.workletNode) {
      try {
        this.workletNode.disconnect();
      } catch {
        /* already gone */
      }
      this.workletNode = null;
    }
    if (this.srcNode) {
      try {
        this.srcNode.disconnect();
      } catch {
        /* already gone */
      }
      this.srcNode = null;
    }
    if (this.ctx) {
      try {
        await this.ctx.close();
      } catch {
        /* already closed */
      }
      this.ctx = null;
    }
    if (this.stream) {
      this.stream.getTracks().forEach((t) => t.stop());
      this.stream = null;
    }
  }
}
