import { Frame, mergeFramePixels } from '../state/document';

export type PreviewBackground = 'light' | 'dark' | 'checkerboard';

export class PreviewRenderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private background: PreviewBackground = 'light';
  private scale: number;
  private animation: boolean = false;
  private animationSpeed: number = 1;
  private frameIndex: number = 0;
  private lastTime: number = 0;

  constructor(canvas: HTMLCanvasElement, scale: number = 1) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d')!;
    this.scale = scale;
  }

  setScale(scale: number): void {
    this.scale = scale;
  }

  setBackground(bg: PreviewBackground): void {
    this.background = bg;
  }

  setAnimationSpeed(speed: number): void {
    this.animationSpeed = speed;
  }

  setAnimation(on: boolean): void {
    this.animation = on;
  }

  reset(): void {
    this.lastTime = 0;
    this.frameIndex = 0;
  }

  renderFrames(frames: Frame[], width: number, height: number): void {
    this.canvas.width = width * this.scale;
    this.canvas.height = height * this.scale;
    this.renderBackground();
    this.drawFrame(frames, width, height);
  }

  tick(frames: Frame[], width: number, height: number, timestamp: number): void {
    if (!this.animation) {
      this.frameIndex = 0;
      this.renderFrames(frames, width, height);
      return;
    }

    if (this.lastTime === 0) {
      this.lastTime = timestamp;
      this.renderFrames(frames, width, height);
      return;
    }

    if (frames.length <= 1) {
      this.renderFrames(frames, width, height);
      return;
    }

    if (this.frameIndex >= frames.length) this.frameIndex = 0;

    const frame = frames[this.frameIndex];
    const delta = timestamp - this.lastTime;
    const threshold = frame.delayMs / this.animationSpeed;

    if (delta >= threshold) {
      this.frameIndex = (this.frameIndex + 1) % frames.length;
      this.lastTime = timestamp;
      this.renderFrames([frames[this.frameIndex]], width, height);
    }
  }

  private renderBackground(): void {
    if (this.background === 'checkerboard') {
      const cell = 8;
      for (let y = 0; y < this.canvas.height; y += cell) {
        for (let x = 0; x < this.canvas.width; x += cell) {
          this.ctx.fillStyle = ((x / cell + y / cell) % 2 === 0) ? '#f0f0f0' : '#c0c0c0';
          this.ctx.fillRect(x, y, cell, cell);
        }
      }
      return;
    }

    this.ctx.fillStyle = this.background === 'light' ? '#f5f5f5' : '#2d2d2d';
    this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
  }

  private drawFrame(frames: Frame[], width: number, height: number): void {
    const frame = frames[0];
    if (!frame) return;

    const merged = mergeFramePixels(frame, width, height);

    const imgData = this.ctx.createImageData(width, height);
    imgData.data.set(merged);

    const off = document.createElement('canvas');
    off.width = width;
    off.height = height;
    const octx = off.getContext('2d')!;
    octx.putImageData(imgData, 0, 0);
    octx.imageSmoothingEnabled = false;
    this.ctx.imageSmoothingEnabled = false;
    this.ctx.drawImage(off, 0, 0, this.canvas.width, this.canvas.height);
  }
}