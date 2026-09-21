// @vitest-environment jsdom
/**
 * Task 16 — the Prop bag as a detached panel.
 *
 * The bag is the one panel whose state does NOT ride `palace://event`: the
 * catalog, shelves, trash and outfits arrive only through the `bag_*` /
 * `outfits_*` commands that `store.loadBag()` gathers. These tests pin the two
 * consequences:
 *
 * 1. The `panel-props` window seeds its own snapshot on mount — it must not
 *    open empty — and renders as a window (no backdrop, no modal semantics).
 * 2. In `main` the modal is the detach entry point; once the bag is detached
 *    the modal turns into a placeholder instead of a second, conflicting copy.
 *
 * Wearing, taking off and the 9-prop cap are asserted from the detached window
 * so the panel cannot silently diverge from the modal's behaviour.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { BAG_CATALOG, unlisten } = vi.hoisted(() => {
  const entry = (id: number) => ({
    id,
    crc: 1000 + id,
    name: `Prop ${id}`,
    w: 44,
    h: 44,
    flags: 0,
    fav: false,
    trash: false,
    collection: "My Bag",
    source: "bag" as const,
  });
  return {
    BAG_CATALOG: Array.from({ length: 10 }, (_, index) => entry(index + 1)),
    unlisten: vi.fn(),
  };
});

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => unlisten),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
    bagCollections: vi.fn(async () => [
      { path: "/bags/My Bag.prp", name: "My Bag", writable: true, exists: true },
    ]),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => BAG_CATALOG),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
  };
});

import { tick, mount, unmount } from "svelte";

import PanelWindow from "./PanelWindow.svelte";
import PropBagDialog from "./components/PropBagDialog.svelte";
import * as api from "./api";
import type { UserInfo } from "./api";
import { panelLayout } from "./panelLayout.svelte";
import { store } from "./store.svelte";

let mounted: ReturnType<typeof mount> | undefined;

// The wear path ends in `set_props`, which needs a Tauri host; the store state
// is what these tests assert, so the IPC call is stubbed like `storeBag.test.ts`.
const setProps = vi.spyOn(store, "setProps").mockImplementation(() => {});

const selfWith = (props: number[]): UserInfo => ({
  id: 1,
  name: "Me",
  face: 0,
  color: 0,
  room_id: 1,
  x: 0,
  y: 0,
  props,
  away: false,
  is_self: true,
  avatar_type: 0,
  avatar_flags: 0,
  avatar_hash: null,
});

function teardown(): void {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
}

/** Wait until the grid shows the whole catalog (proves the seed landed). */
async function waitForTiles(): Promise<void> {
  await vi.waitFor(() =>
    expect(document.querySelectorAll(".bag-tile").length).toBe(BAG_CATALOG.length),
  );
}

/** Mount the real `panel-props` window: hash route plus PanelWindow. */
async function renderPanelWindow(): Promise<void> {
  window.location.hash = "#/panel/props";
  mounted = mount(PanelWindow, { target: document.body, props: { panel: "props" } });
  await tick();
}

/** Mount the bag the way `Viewport.svelte` does in `main`. */
async function renderModal(): Promise<ReturnType<typeof vi.fn>> {
  const onclose = vi.fn();
  mounted = mount(PropBagDialog, { target: document.body, props: { onclose } });
  await tick();
  return onclose;
}

beforeEach(() => {
  vi.clearAllMocks();
  setProps.mockClear();
  panelLayout.reset();
  store.bagLoaded = false;
  store.users = [];
  window.location.hash = "";
  document.body.innerHTML = "";
});

afterEach(() => {
  teardown();
  window.location.hash = "";
});

describe("the detached Prop bag window", () => {
  it("seeds its own bag snapshot on mount and renders as a window, not a modal", async () => {
    await renderPanelWindow();
    await waitForTiles();

    // Bag state does not ride palace://event — the panel fetched its own copy.
    expect(vi.mocked(api.bagCollections)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.bagShelves)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.bagCatalog)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.bagTrashList)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.outfitsList)).toHaveBeenCalledTimes(1);

    // A window, not a modal: no backdrop, no aria-modal, no detach control.
    expect(document.querySelector(".bag-panel")).not.toBeNull();
    expect(document.querySelector(".dialog-backdrop")).toBeNull();
    expect(document.querySelector("[aria-modal]")).toBeNull();
    expect(document.querySelector("[data-detach]")).toBeNull();
  });

  it("wears and takes off from the detached window, keeping the worn count correct", async () => {
    store.users = [selfWith([1, 2, 3, 4, 5, 6, 7, 8])];
    await renderPanelWindow();
    await waitForTiles();
    expect(document.querySelector(".prop-count")?.textContent).toContain("8 / 9");

    // Entry 9 or 10 is not worn yet: clicking its tile wears it.
    document
      .querySelector<HTMLButtonElement>(".bag-tile:not(.worn) .bag-tile-wear")
      ?.click();
    await vi.waitFor(() =>
      expect(document.querySelector(".prop-count")?.textContent).toContain("9 / 9"),
    );
    expect(store.self?.props).toHaveLength(9);

    // Clicking a worn tile takes that prop back off.
    const worn = document.querySelectorAll<HTMLButtonElement>(".bag-tile.worn .bag-tile-wear");
    worn[worn.length - 1]?.click();
    await vi.waitFor(() =>
      expect(document.querySelector(".prop-count")?.textContent).toContain("8 / 9"),
    );
  });

  it("refuses a tenth prop at the 9-prop cap", async () => {
    store.users = [selfWith([1, 2, 3, 4, 5, 6, 7, 8, 9])];
    await renderPanelWindow();
    await waitForTiles();
    expect(document.querySelector(".prop-count")?.textContent).toContain("9 / 9");

    document
      .querySelector<HTMLButtonElement>(".bag-tile:not(.worn) .bag-tile-wear")
      ?.click();

    await vi.waitFor(() => expect(document.querySelector(".dialog-error")).not.toBeNull());
    expect(document.querySelector(".dialog-error")?.textContent).toContain("9");
    expect(store.self?.props).toHaveLength(9);
    expect(document.querySelector(".prop-count")?.textContent).toContain("9 / 9");
  });
});

describe("detaching and re-attaching from the main window", () => {
  it("sends open_panel('props') and swaps the modal for a placeholder", async () => {
    await renderModal();
    await waitForTiles();
    expect(document.querySelector("[aria-modal='true']")).not.toBeNull();

    document.querySelector<HTMLButtonElement>("[data-detach='props']")?.click();
    await vi.waitFor(() => expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("props"));

    // No second copy in main: the modal becomes the re-attach placeholder.
    await vi.waitFor(() => expect(document.querySelector(".bag-detached")).not.toBeNull());
    expect(document.querySelector(".bag-layout")).toBeNull();
    expect(document.querySelector(".bag-detached")?.textContent).toContain("its own window");

    // Re-attaching closes the panel window and brings the live bag back.
    document
      .querySelector<HTMLButtonElement>(".bag-detached .dialog-actions button")
      ?.click();
    await vi.waitFor(() => expect(vi.mocked(api.closePanel)).toHaveBeenCalledWith("props"));
    await waitForTiles();
    expect(document.querySelector(".bag-detached")).toBeNull();
  });

  it("shows the placeholder instead of a second copy when the bag is already detached", async () => {
    panelLayout.setDetached("props", true);
    await renderModal();
    await tick();

    expect(document.querySelector(".bag-detached")).not.toBeNull();
    expect(document.querySelector(".bag-layout")).toBeNull();
    // The placeholder does not fetch a snapshot it cannot show.
    expect(vi.mocked(api.bagCatalog)).not.toHaveBeenCalled();
  });
});
