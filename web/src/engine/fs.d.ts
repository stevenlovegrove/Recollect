// Parts of the File System Access API that TypeScript's DOM library doesn't have yet.

type PermissionMode = { mode?: "read" | "readwrite" };

interface FileSystemHandle {
  queryPermission?(descriptor?: PermissionMode): Promise<PermissionState>;
  requestPermission?(descriptor?: PermissionMode): Promise<PermissionState>;
}

interface Window {
  showDirectoryPicker?(options?: { id?: string; mode?: "read" | "readwrite"; startIn?: string }): Promise<FileSystemDirectoryHandle>;
}

interface FileSystemChangeRecord {
  changedHandle: FileSystemHandle | null;
  relativePathComponents: string[];
  type: "appeared" | "disappeared" | "modified" | "moved" | "unknown" | "errored";
}

declare class FileSystemObserver {
  constructor(callback: (records: FileSystemChangeRecord[], observer: FileSystemObserver) => void);
  observe(handle: FileSystemHandle, options?: { recursive?: boolean }): Promise<void>;
  disconnect(): void;
}
