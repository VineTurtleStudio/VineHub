import { Frame, mergeFramePixels } from '../state/document';

export type CanvasBackground = 'checkerboard' | 'white' | 'black';

const CHECKER_A = '#ffffff';
const CHECKER_B = '#cccccc';

export class CanvasRenderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private scale: number = 8;
  private background: CanvasBackground = 'checkerboard';
  private showGrid: boolean = true;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d')!;
  }

  setScale(scale: number): void {
    this.scale = scale;
  }

  getScale(): number {
    return this.scale;
  }

  setBackground(bg: CanvasBackground): void {
    this.background = bg;
  }

  setShowGrid(show: boolean): void {
    this.showGrid = show;
  }

  render(
    frames: Frame[],
    activeIndex: number,
    width: number,
    height: number,
    showOnion: boolean = false,
  ): void {
    const w = width * this.scale;
    const h = height * this.scale;

    this.canvas.width = w;
    this.canvas.height = h;
    this.ctx.imageSmoothingEnabled = false;

    this.drawBackground(width, height);

    if (showOnion) {
      for (const idx of [activeIndex - 1, activeIndex + 1]) {
        const frame = frames[idx];
        if (frame) this.drawFrame(frame, width, height, 0.25, false);
      }
    }

    const frame = frames[activeIndex];
    if (frame) this.drawFrame(frame, width, height, 1, true);
    this.drawGrid(width, height);
  }

  private drawBackground(width: number, height: number): void {
    if (this.background === 'white') {
      this.ctx.fillStyle = '#ffffff';
      this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
      return;
    }
    if (this.background === 'black') {
      this.ctx.fillStyle = '#1a1a1a';
      this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
      return;
    }

    const cell = this.scale;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        this.ctx.fillStyle = (x + y) % 2 === 0 ? CHECKER_A : CHECKER_B;
        this.ctx.fillRect(x * cell, y * cell, cell, cell);
      }
    }
  }

  private drawFrame(
    frame: Frame,
    width: number,
    height: number,
    opacity: number,
    withHotspot: boolean,
  ): void {
    const merged = mergeFramePixels(frame, width, height);

    const imageData = this.ctx.createImageData(width, height);
    imageData.data.set(merged);

    const offscreen = document.createElement('canvas');
    offscreen.width = width;
    offscreen.height = height;
    const octx = offscreen.getContext('2d')!;
    octx.putImageData(imageData, 0, 0);
    octx.imageSmoothingEnabled = false;

    if (opacity !== 1) this.ctx.globalAlpha = opacity;
    this.ctx.imageSmoothingEnabled = false;
    this.ctx.drawImage(offscreen, 0, 0, this.canvas.width, this.canvas.height);
    if (opacity !== 1) this.ctx.globalAlpha = 1;

    if (withHotspot) this.drawHotspot(frame, width, height);
  }

  private drawHotspot(frame: Frame, width: number, height: number): void {
    const hx = frame.hotspotX * this.scale + this.scale / 2;
    const hy = frame.hotspotY * this.scale + this.scale / 2;

    this.ctx.strokeStyle = '#ff4444';
    this.ctx.lineWidth = 2;
    this.ctx.beginPath();
    this.ctx.arc(hx, hy, this.scale * 0.6, 0, Math.PI * 2);
    this.ctx.stroke();

    this.ctx.strokeStyle = '#ff0000';
    this.ctx.lineWidth = 1;
    this.ctx.beginPath();
    this.ctx.moveTo(hx, hy - this.scale * 0.8);
    this.ctx.lineTo(hx, hy + this.scale * 0.8);
    this.ctx.moveTo(hx - this.scale * 0.8, hy);
    this.ctx.lineTo(hx + this.scale * 0.8, hy);
    this.ctx.stroke();
  }

  private drawGrid(width: number, height: number): void {
    if (!this.showGrid || this.scale < 4 || this.scale > 64) return;

    this.ctx.strokeStyle = 'rgba(0, 0, 0, 0.08)';
    this.ctx.lineWidth = 1;
    this.ctx.beginPath();

    for (let x = 0; x <= width; x++) {
      const px = x * this.scale + 0.5;
      this.ctx.moveTo(px, 0);
      this.ctx.lineTo(px, this.canvas.height);
    }
    for (let y = 0; y <= height; y++) {
      const py = y * this.scale + 0.5;
      this.ctx.moveTo(0, py);
      this.ctx.lineTo(this.canvas.width, py);
    }
    this.ctx.stroke();
  }

  screenToImage(screenX: number, screenY: number): { x: number; y: number } {
    const rect = this.canvas.getBoundingClientRect();
    const x = Math.floor((screenX - rect.left) / this.scale);
    const y = Math.floor((screenY - rect.top) / this.scale);
    return { x, y };
  }

  exportPng(frame: Frame, width: number, height: number): string {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d')!;
    ctx.clearRect(0, 0, width, height);

    const imageData = ctx.createImageData(width, height);
    for (let i = 0; i < width * height; i++) {
      imageData.data[i * 4] = frame.layers[0].pixels[i * 4];
      imageData.data[i * 4 + 1] = frame.layers[0].pixels[i * 4 + 1];
      imageData.data[i * 4 + 2] = frame.layers[0].pixels[i * 4 + 2];
      imageData.data[i * 4 + 3] = frame.layers[0].pixels[i * 4 + 3];
    }
    ctx.putImageData(imageData, 0, 0);

    return canvas.toDataURL('image/png');
  }
}