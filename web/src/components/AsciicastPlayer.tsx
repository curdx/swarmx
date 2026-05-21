/**
 * AsciicastPlayer — thin React wrapper around the official `asciinema-player`
 * package. The package itself is framework-agnostic (vanilla JS + WASM
 * payload), so we own the lifecycle: create on mount, dispose on unmount or
 * src change. Re-creating on src change is deliberately the only re-init
 * path — the player has no live `setSrc` API.
 *
 * Why we don't use the community `react-asciinema-player` wrapper:
 *   - last published 2023-03, unmaintained.
 *   - doesn't track the new 3.x WASM-based renderer.
 *
 * Options we expose intentionally:
 *   - `src`: URL to a `.cast` file (asciicast v2). The server already serves
 *     these at `/api/recording/<id>/cast`.
 *   - `cols` / `rows`: rendered terminal dimensions; pulled from the
 *     recording metadata so playback matches recording geometry.
 *   - `autoPlay`: default off so opening multiple rows doesn't fire a wall
 *     of concurrent decoders.
 *   - `theme`: "asciinema" (player default) — matches the dark UI.
 */

import { useEffect, useRef } from "react";
import * as AsciinemaPlayer from "asciinema-player";
import "asciinema-player/dist/bundle/asciinema-player.css";

interface Props {
  src: string;
  cols?: number;
  rows?: number;
  autoPlay?: boolean;
}

export function AsciicastPlayer({ src, cols, rows, autoPlay = false }: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!hostRef.current) return;
    const host = hostRef.current;
    // create() returns a player handle whose .dispose() tears down the
    // canvas/WASM/event listeners. Calling create with the host element
    // appends the player DOM under it.
    const player = AsciinemaPlayer.create(src, host, {
      cols,
      rows,
      autoPlay,
      // Speed up long idle gaps so a recorded LLM "think" pause doesn't
      // make the user wait 30s. 2s cap is the common asciinema default.
      idleTimeLimit: 2,
      // fit="width" scales the font so the recording fills the parent's
      // inline-size; height follows. We deliberately avoid fit:"both"
      // because it requires the parent to have a known block-size up
      // front, and our flex sidebar doesn't (it auto-sizes to children).
      fit: "width",
      theme: "asciinema",
    });
    return () => {
      try {
        player.dispose();
      } catch {
        /* second dispose if React StrictMode double-invokes — harmless */
      }
    };
    // Re-mount the player when `src` changes (different recording row).
    // cols/rows/autoPlay changes don't need a re-mount in practice.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src]);

  return <div ref={hostRef} style={hostStyle} />;
}

const hostStyle: React.CSSProperties = {
  // The player honours `fit: "width"` against this box's inline-size, so
  // we MUST give it a full-width block element. Letting the player decide
  // its own width (the default) collapses to 0 inside a flex column
  // unless the parent explicitly sets align-items: stretch.
  width: "100%",
  background: "#0d0d0d",
  borderRadius: 4,
  overflow: "hidden",
};
