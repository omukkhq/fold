export type SseFrame =
  | { kind: "comment"; text: string }
  | { kind: "message"; id: string | null; event: string | null; data: string };

export interface SseParser {
  push(chunk: string): SseFrame[];
}

/**
 * A line ending of `\r` at the very end of the buffer is held back until the next chunk, because it may be the first half of `\r\n`.
 */
export function createSseParser(): SseParser {
  let buffer = "";
  let id: string | null = null;
  let event: string | null = null;
  let data: string[] = [];

  function handleLine(line: string): SseFrame | null {
    if (line === "") {
      if (data.length === 0) {
        id = null;
        event = null;
        return null;
      }
      const frame: SseFrame = {
        kind: "message",
        id,
        event,
        data: data.join("\n"),
      };
      id = null;
      event = null;
      data = [];
      return frame;
    }
    if (line.startsWith(":")) {
      return { kind: "comment", text: line.slice(1).trimStart() };
    }
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) {
      value = value.slice(1);
    }
    if (field === "id") {
      id = value;
    } else if (field === "event") {
      event = value;
    } else if (field === "data") {
      data.push(value);
    }
    return null;
  }

  return {
    push(chunk) {
      buffer += chunk;
      const frames: SseFrame[] = [];
      let start = 0;
      let i = 0;
      while (i < buffer.length) {
        const c = buffer[i];
        if (c !== "\n" && c !== "\r") {
          i += 1;
          continue;
        }
        if (c === "\r" && i + 1 === buffer.length) {
          break;
        }
        const line = buffer.slice(start, i);
        i += c === "\r" && buffer[i + 1] === "\n" ? 2 : 1;
        start = i;
        const frame = handleLine(line);
        if (frame) {
          frames.push(frame);
        }
      }
      buffer = buffer.slice(start);
      return frames;
    },
  };
}
