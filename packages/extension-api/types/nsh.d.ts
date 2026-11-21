declare module "nsh" {
  export interface HostCallOptions {
    timeout?: number;
  }

  export interface HostBridge {
    params: Record<string, unknown>;
    on(event: string, listener: (value: unknown) => void): () => void;
    call<T = unknown>(name: string, data?: unknown, options?: HostCallOptions): Promise<T>;
    post(name: string, data?: unknown): void;
  }

  export const host: HostBridge;
}
