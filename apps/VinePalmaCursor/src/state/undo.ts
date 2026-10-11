import { DocumentModel, cloneDocument } from './document';

const MAX_UNDO_STACK = 50;

export class UndoManager {
  private undoStack: DocumentModel[] = [];
  private redoStack: DocumentModel[] = [];
  private undoHistory: string[] = [];
  private redoHistory: string[] = [];

  pushSnapshot(doc: DocumentModel, label = '编辑'): void {
    this.undoStack.push(cloneDocument(doc));
    this.undoHistory.push(label);
    if (this.undoStack.length > MAX_UNDO_STACK) {
      this.undoStack.shift();
      this.undoHistory.shift();
    }
    this.redoStack = [];
    this.redoHistory = [];
  }

  undo(doc: DocumentModel): DocumentModel | null {
    if (this.undoStack.length === 0) return null;
    this.redoStack.push(cloneDocument(doc));
    const label = this.undoHistory.pop() ?? '编辑';
    this.redoHistory.push(label);
    return this.undoStack.pop()!;
  }

  redo(doc: DocumentModel): DocumentModel | null {
    if (this.redoStack.length === 0) return null;
    this.undoStack.push(cloneDocument(doc));
    const label = this.redoHistory.pop() ?? '编辑';
    this.undoHistory.push(label);
    return this.redoStack.pop()!;
  }

  canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  undoCount(): number {
    return this.undoStack.length;
  }

  redoCount(): number {
    return this.redoStack.length;
  }

  undoLabelsTopFirst(): string[] {
    return [...this.undoHistory].reverse();
  }

  redoLabels(): string[] {
    return [...this.redoHistory];
  }

  clear(): void {
    this.undoStack = [];
    this.redoStack = [];
    this.undoHistory = [];
    this.redoHistory = [];
  }
}