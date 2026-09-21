/**
 * The Chat logging group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority: `to_file` defaults to false,
 * `path` to `null`, `max_bytes` to 8 MiB and `rotate_files` to 3. These tests
 * pin the resolver, the range checks and the support table, so a component that
 * drifts from the specification fails loudly.
 */
import { describe, expect, it } from "vitest";

import {
  CHAT_LOG_OPTIONS,
  DEFAULT_MAX_BYTES,
  DEFAULT_PATH,
  DEFAULT_ROTATE_FILES,
  DEFAULT_TO_FILE,
  IN_MEMORY_LOG_LINES,
  MAX_MAX_BYTES,
  MAX_ROTATE_FILES,
  MIN_MAX_BYTES,
  UNSUPPORTED_CHAT_LOG_KEYS,
  chatLogOption,
  chatLogPrefsFrom,
  formatBytes,
  isChatLogOptionSupported,
  isMaxBytesInRange,
  isRotateFilesInRange,
} from "./chatLogPrefs";

describe("the chat log defaults", () => {
  it("match the specification", () => {
    expect(DEFAULT_TO_FILE).toBe(false);
    expect(DEFAULT_PATH).toBeNull();
    expect(DEFAULT_MAX_BYTES).toBe(8 * 1024 * 1024);
    expect(DEFAULT_ROTATE_FILES).toBe(3);
    expect(IN_MEMORY_LOG_LINES).toBe(500);
  });

  it("are used when the stored block is absent or empty", () => {
    expect(chatLogPrefsFrom({})).toEqual({
      toFile: false,
      path: null,
      maxBytes: DEFAULT_MAX_BYTES,
      rotateFiles: DEFAULT_ROTATE_FILES,
    });
    expect(chatLogPrefsFrom({ chat_log: {} })).toEqual({
      toFile: false,
      path: null,
      maxBytes: DEFAULT_MAX_BYTES,
      rotateFiles: DEFAULT_ROTATE_FILES,
    });
  });

  it("resolve stored values, trimming the destination", () => {
    const resolved = chatLogPrefsFrom({
      chat_log: {
        to_file: true,
        path: "  $HOME/room.log  ",
        max_bytes: 1024 * 1024,
        rotate_files: 0,
      },
    });
    expect(resolved).toEqual({
      toFile: true,
      path: "$HOME/room.log",
      maxBytes: 1024 * 1024,
      rotateFiles: 0,
    });
  });

  it("fall back per key when a stored value has the wrong type", () => {
    const resolved = chatLogPrefsFrom({
      chat_log: {
        to_file: "yes",
        path: 7,
        max_bytes: "big",
        rotate_files: -1,
      },
    });
    expect(resolved).toEqual({
      toFile: false,
      path: null,
      maxBytes: DEFAULT_MAX_BYTES,
      rotateFiles: DEFAULT_ROTATE_FILES,
    });
  });

  it("fall back per key when a stored number is outside the accepted range", () => {
    expect(chatLogPrefsFrom({ chat_log: { max_bytes: 1 } }).maxBytes).toBe(DEFAULT_MAX_BYTES);
    expect(chatLogPrefsFrom({ chat_log: { max_bytes: MAX_MAX_BYTES + 1 } }).maxBytes).toBe(
      DEFAULT_MAX_BYTES,
    );
    expect(chatLogPrefsFrom({ chat_log: { max_bytes: 1.5 } }).maxBytes).toBe(DEFAULT_MAX_BYTES);
    expect(chatLogPrefsFrom({ chat_log: { rotate_files: MAX_ROTATE_FILES + 1 } }).rotateFiles).toBe(
      DEFAULT_ROTATE_FILES,
    );
  });

  it("treat a blank destination as the default location", () => {
    expect(chatLogPrefsFrom({ chat_log: { path: "   " } }).path).toBeNull();
    expect(chatLogPrefsFrom({ chat_log: { path: null } }).path).toBeNull();
  });
});

describe("the accepted ranges", () => {
  it("include the endpoints and exclude what is outside them", () => {
    expect(isMaxBytesInRange(MIN_MAX_BYTES)).toBe(true);
    expect(isMaxBytesInRange(MAX_MAX_BYTES)).toBe(true);
    expect(isMaxBytesInRange(MIN_MAX_BYTES - 1)).toBe(false);
    expect(isMaxBytesInRange(MAX_MAX_BYTES + 1)).toBe(false);
    expect(isMaxBytesInRange(1.5)).toBe(false);

    expect(isRotateFilesInRange(0)).toBe(true);
    expect(isRotateFilesInRange(MAX_ROTATE_FILES)).toBe(true);
    expect(isRotateFilesInRange(-1)).toBe(false);
    expect(isRotateFilesInRange(MAX_ROTATE_FILES + 1)).toBe(false);
    expect(isRotateFilesInRange(2.5)).toBe(false);
  });
});

describe("the byte formatter", () => {
  it("names the largest whole unit that divides the size", () => {
    expect(formatBytes(8 * 1024 * 1024)).toBe("8 MiB");
    expect(formatBytes(1024 * 1024)).toBe("1 MiB");
    expect(formatBytes(4 * 1024)).toBe("4 KiB");
    expect(formatBytes(1500)).toBe("1500 B");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-1)).toBe("0 B");
  });
});

describe("the support table", () => {
  it("names the four stored keys in display order", () => {
    expect(CHAT_LOG_OPTIONS.map((option) => option.key)).toEqual([
      "to_file",
      "path",
      "max_bytes",
      "rotate_files",
    ]);
  });

  it("marks every option supported, because the writer backs all four", () => {
    expect(UNSUPPORTED_CHAT_LOG_KEYS).toEqual([]);
    for (const option of CHAT_LOG_OPTIONS) {
      expect(option.support, option.key).toBe("supported");
      expect(option.reason, option.key).toBeNull();
      expect(isChatLogOptionSupported(option.key), option.key).toBe(true);
      expect(chatLogOption(option.key)).toBe(option);
    }
  });

  it("does not claim support for a key the backend does not read", () => {
    expect(chatLogOption("password")).toBeNull();
    expect(isChatLogOptionSupported("password")).toBe(false);
    expect(isChatLogOptionSupported("")).toBe(false);
  });
});
