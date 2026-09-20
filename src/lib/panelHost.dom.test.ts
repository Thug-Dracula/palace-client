// @vitest-environment jsdom
/**
 * The panel host: one abstraction that either renders the live panel for an id
 * or, when that panel currently lives in its own OS window, a placeholder with
 * a working re-attach control.
 *
 * The id -> component mapping is the seam every later detach task binds to, so
 * each of the five ids is asserted to render its component and no other.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    closePanel: vi.fn(async () => "closed"),
    setViewport: vi.fn(async () => {}),
    // The prop bag panel loads its own snapshot on mount.
    bagCollections: vi.fn(async () => []),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => []),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
  };
});

import { tick, mount, unmount } from "svelte";

import PanelHost from "./PanelHost.svelte";
import * as api from "./api";
import type { PanelId } from "./panels";

/* jsdom does not implement ResizeObserver; the room view uses one to report
   its measured size. A no-op observer keeps the mount path real. */
class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

async function renderHost(panel: PanelId, detached = false): Promise<void> {
  mounted = mount(PanelHost, { target: document.body, props: { panel, detached } });
  await tick();
}

function sidebarHead(): string | undefined {
  return document.querySelector(".sidebar .panel-head span")?.textContent?.trim();
}

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("panel host view selection", () => {
  it("renders the Room view for room and nothing else", async () => {
    await renderHost("room");
    expect(document.querySelector(".viewport-wrap")).not.toBeNull();
    expect(document.querySelector(".sidebar")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
    expect(document.querySelector(".bag-dialog")).toBeNull();
  });

  it("renders only the Users list for users", async () => {
    await renderHost("users");
    expect(sidebarHead()).toBe("In room");
    expect(document.querySelector(".viewport-wrap")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
    expect(document.querySelector(".bag-dialog")).toBeNull();
  });

  it("renders only the Rooms list for rooms", async () => {
    await renderHost("rooms");
    expect(sidebarHead()).toBe("Rooms");
    expect(document.querySelector(".viewport-wrap")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
    expect(document.querySelector(".bag-dialog")).toBeNull();
  });

  it("renders only the chat log for chat", async () => {
    await renderHost("chat");
    expect(document.querySelector("section.chat")).not.toBeNull();
    expect(document.querySelector(".viewport-wrap")).toBeNull();
    expect(document.querySelector(".sidebar")).toBeNull();
    expect(document.querySelector(".bag-dialog")).toBeNull();
  });

  it("renders only the prop bag for props", async () => {
    await renderHost("props");
    expect(document.querySelector(".bag-dialog")).not.toBeNull();
    expect(document.querySelector(".viewport-wrap")).toBeNull();
    expect(document.querySelector(".sidebar")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
  });
});

describe("panel host detached placeholder", () => {
  it("replaces the live panel with a re-attach placeholder", async () => {
    await renderHost("users", true);
    const placeholder = document.querySelector(".panel-placeholder");
    expect(placeholder).not.toBeNull();
    expect(placeholder?.textContent).toContain("This panel is in its own window");
    expect(placeholder?.querySelector("button")?.textContent?.trim()).toBe("Reattach");
    // The live panel is not rendered twice while it is detached.
    expect(document.querySelector(".sidebar")).toBeNull();
  });

  it("re-attaches through the Rust close_panel command when the control is used", async () => {
    await renderHost("users", true);
    document.querySelector<HTMLButtonElement>(".panel-placeholder button")?.click();
    await vi.waitFor(() => expect(vi.mocked(api.closePanel)).toHaveBeenCalledWith("users"));
  });

  it("names the panel it stands in for", async () => {
    await renderHost("chat", true);
    const placeholder = document.querySelector(".panel-placeholder");
    expect(placeholder?.getAttribute("data-panel")).toBe("chat");
    document.querySelector<HTMLButtonElement>(".panel-placeholder button")?.click();
    await vi.waitFor(() => expect(vi.mocked(api.closePanel)).toHaveBeenCalledWith("chat"));
  });
});
