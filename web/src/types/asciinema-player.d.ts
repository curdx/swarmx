// Minimal ambient types for `asciinema-player` 3.x. The package ships
// untyped JS; this declaration covers only the surface we touch
// (`create` factory + the handle's `dispose` method + a couple of options).
// Full API is at https://docs.asciinema.org/manual/player/api/ — extend
// this as we use more of it.

declare module "asciinema-player" {
  export interface CreateOptions {
    cols?: number;
    rows?: number;
    autoPlay?: boolean;
    loop?: boolean | number;
    speed?: number;
    idleTimeLimit?: number;
    fit?: "false" | "width" | "height" | "both";
    theme?: string;
    preload?: boolean;
    startAt?: number | string;
    poster?: string;
    pauseOnMarkers?: boolean;
  }

  export interface PlayerHandle {
    dispose: () => void;
    play: () => Promise<void>;
    pause: () => void;
    seek: (where: number | string) => Promise<void>;
    getCurrentTime: () => number;
    getDuration: () => number | null;
    addEventListener: (event: string, handler: (...args: unknown[]) => void) => void;
  }

  export function create(
    src: string | { url: string },
    el: HTMLElement,
    opts?: CreateOptions,
  ): PlayerHandle;
}

declare module "asciinema-player/dist/bundle/asciinema-player.css";
