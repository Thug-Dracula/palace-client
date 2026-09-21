// @vitest-environment jsdom
/**
 * Task 20's acceptance tests: the Connection & identity preference group.
 *
 * The plan names four behaviours, and each one has a test here that can fail:
 *
 * * the last-servers list keeps at most eight entries, most-recent-first;
 * * the home palace persists and the launch offer is made (and can be refused);
 * * changing the host marks a reconnect as required and never reconnects;
 * * no password is ever written into the settings document.
 *
 * The DOM tests mount the real group inside the real Preferences shell, with
 * only the `api` module mocked, so the save path under test is the one the
 * window actually runs.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => {
  const isRecord = (value: unknown): value is Record<string, unknown> =>
    typeof value === "object" && value !== null && !Array.isArray(value);
  // The backend's `set_prefs` merges nested objects and replaces arrays; the
  // mock mirrors that so a connection patch cannot wipe a sibling key.
  const merge = (
    base: Record<string, unknown>,
    patch: Record<string, unknown>,
  ): Record<string, unknown> => {
    const out: Record<string, unknown> = { ...base };
    for (const [key, value] of Object.entries(patch)) {
      out[key] = isRecord(value) && isRecord(out[key]) ? merge(out[key], value) : value;
    }
    return out;
  };
  return {
    stored: {} as Record<string, unknown>,
    failNextSetPrefs: false,
    merge,
  };
});

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
      identity: {
        registration: { crc: 0x01020304, counter: 0x0a0b0c0d },
        puid: { ctr: 0x11223344, crc: 0x55667788 },
      },
    })),
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      if (state.failNextSetPrefs) {
        state.failNextSetPrefs = false;
        throw new Error("the disk is full");
      }
      state.stored = state.merge(state.stored, patch);
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = { shell: { last_group: "connection" } };
      return state.stored;
    }),
    setConnectionSettings: vi.fn(async (host: string, port: number, username: string) => ({
      host,
      port,
      username,
      soundfont: null,
    })),
    openPreferences: vi.fn(async () => "created"),
    closePreferences: vi.fn(async () => "closed"),
    connect: vi.fn(async () => {}),
    disconnect: vi.fn(async () => {}),
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import App from "./App.svelte";
import * as api from "./api";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import HomePalaceOffer from "./components/HomePalaceOffer.svelte";
import {
  autoConnectFrom,
  describeServer,
  forgetServer,
  homePalaceFrom,
  identityFingerprint,
  lastServersFrom,
  launchOffer,
  rememberServer,
  type ServerRef,
} from "./connectionPrefs";
import { prefs } from "./prefsStore.svelte";
import { store } from "./store.svelte";

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

function server(host: string, port = 9998, username = "Guest"): ServerRef {
  return { host, port, username };
}

function storedConnection(): {
  last_servers?: ServerRef[];
  home_palace?: ServerRef | null;
  auto_connect?: boolean;
} {
  return (state.stored.connection ?? {}) as {
    last_servers?: ServerRef[];
    home_palace?: ServerRef | null;
    auto_connect?: boolean;
  };
}

async function renderPreferences(): Promise<void> {
  mounted = mount(PreferencesWindow, { target: document.body });
  await tick();
  await vi.waitFor(() => expect(document.querySelector(".prefs-group")).not.toBeNull());
  await vi.waitFor(() =>
    expect(document.querySelector<HTMLInputElement>(".prefs-field input")?.value).toBe(
      "localhost",
    ),
  );
}

function buttonByText(text: string): HTMLButtonElement {
  const button = [...document.querySelectorAll<HTMLButtonElement>("button")].find((node) =>
    node.textContent?.trim().startsWith(text),
  );
  if (!button) {
    throw new Error(`no button starts with ${text}`);
  }
  return button;
}

async function changeField(input: HTMLInputElement, value: string): Promise<void> {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.failNextSetPrefs = false;
  prefs.reset();
  store.status = "disconnected";
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("the last-servers rules", () => {
  it("keeps at most eight entries, newest first", () => {
    let list: ServerRef[] = [];
    for (let index = 0; index < 11; index += 1) {
      list = rememberServer(list, server(`s${index}.test`));
    }

    expect(list).toHaveLength(8);
    expect(list.map((entry) => entry.host)).toEqual([
      "s10.test",
      "s9.test",
      "s8.test",
      "s7.test",
      "s6.test",
      "s5.test",
      "s4.test",
      "s3.test",
    ]);
  });

  it("treats the same palace as one row and keeps the latest name", () => {
    let list = rememberServer([], server("localhost", 9998, "Guest"));
    list = rememberServer(list, server("LIBRARY.lan", 9998, "Dracula"));

    expect(list).toEqual([server("LIBRARY.lan", 9998, "Dracula")]);
  });

  it("ignores unusable entries and caps a hand-edited file", () => {
    const raw = [
      server("one.test"),
      { host: "bad-port.test", port: 0, username: "Guest" },
      { host: "", port: 9998, username: "Guest" },
      server("two.test"),
      server("three.test"),
      server("four.test"),
      server("five.test"),
      server("six.test"),
      server("seven.test"),
      server("eight.test"),
      server("nine.test"),
    ];
    const values = { connection: { last_servers: raw } };

    const list = lastServersFrom(values);
    expect(list).toHaveLength(8);
    expect(list.map((entry) => entry.host)).toEqual([
      "one.test",
      "two.test",
      "three.test",
      "four.test",
      "five.test",
      "six.test",
      "seven.test",
      "eight.test",
    ]);
  });

  it("forgets one palace without disturbing the rest", () => {
    const list = [server("one.test"), server("two.test"), server("three.test")];
    expect(forgetServer(list, server("TWO.test"))).toEqual([
      server("one.test"),
      server("three.test"),
    ]);
  });
});

describe("the launch offer rules", () => {
  it("needs both the home palace and the auto-connect switch", () => {
    const palace = server("home.test");
    expect(launchOffer({ connection: { home_palace: palace } })).toBeNull();
    expect(launchOffer({ connection: { auto_connect: true } })).toBeNull();
    expect(
      launchOffer({ connection: { home_palace: palace, auto_connect: true } }),
    ).toEqual(palace);
  });

  it("reads the home palace and switch defensively", () => {
    expect(homePalaceFrom({ connection: { home_palace: { host: "", port: 1 } } })).toBeNull();
    expect(homePalaceFrom({})).toBeNull();
    expect(autoConnectFrom({ connection: { auto_connect: "yes" } })).toBe(false);
    expect(autoConnectFrom({ connection: { auto_connect: true } })).toBe(true);
  });
});

describe("the identity indicator rules", () => {
  it("prints the PUID and refuses an unrecognised shape", () => {
    expect(
      identityFingerprint({ puid: { ctr: 0x11223344, crc: 0x55667788 } }),
    ).toBe("11223344-55667788");
    expect(identityFingerprint(null)).toBeNull();
    expect(identityFingerprint({ puid: "nope" })).toBeNull();
    expect(identityFingerprint({})).toBeNull();
  });
});

describe("the group's save path", () => {
  it("keeps at most eight servers, newest first, in the saved document", async () => {
    await renderPreferences();
    const host = document.querySelector<HTMLInputElement>(".prefs-field input") as HTMLInputElement;

    for (let index = 0; index < 10; index += 1) {
      await changeField(host, `s${index}.test`);
      await vi.waitFor(() =>
        expect(storedConnection().last_servers?.[0]?.host).toBe(`s${index}.test`),
      );
    }

    const list = storedConnection().last_servers ?? [];
    expect(list).toHaveLength(8);
    expect(list.map((entry) => entry.host)).toEqual([
      "s9.test",
      "s8.test",
      "s7.test",
      "s6.test",
      "s5.test",
      "s4.test",
      "s3.test",
      "s2.test",
    ]);
  });

  it("loads a remembered server into the fields and moves it to the front", async () => {
    state.stored = {
      connection: { last_servers: [server("one.test"), server("two.test")] },
    };
    await renderPreferences();

    const rows = document.querySelectorAll<HTMLButtonElement>(".prefs-server-load");
    expect(rows).toHaveLength(2);
    rows[1].click();

    await vi.waitFor(() => expect(storedConnection().last_servers?.[0]?.host).toBe("two.test"));
    expect(document.querySelector<HTMLInputElement>(".prefs-field input")?.value).toBe("two.test");
    expect(storedConnection().last_servers).toHaveLength(2);
  });

  it("forgets a server and saves the shortened list", async () => {
    state.stored = { connection: { last_servers: [server("one.test"), server("two.test")] } };
    await renderPreferences();

    const forgetButtons = [...document.querySelectorAll<HTMLButtonElement>(".prefs-server-forget")];
    expect(forgetButtons).toHaveLength(2);
    forgetButtons[0].click();

    await vi.waitFor(() =>
      expect(storedConnection().last_servers?.map((entry) => entry.host)).toEqual(["two.test"]),
    );
  });

  it("requires a reconnect for a host change and never reconnects on its own", async () => {
    await renderPreferences();
    const host = document.querySelector<HTMLInputElement>(".prefs-field input") as HTMLInputElement;

    await changeField(host, "elsewhere.test");

    await vi.waitFor(() =>
      expect(vi.mocked(api.setConnectionSettings)).toHaveBeenCalledWith(
        "elsewhere.test",
        9998,
        "Guest",
      ),
    );
    await vi.waitFor(() => expect(document.querySelector(".prefs-reconnect")).not.toBeNull());
    expect(document.querySelector(".prefs-reconnect-chip")?.textContent).toBe("Reconnect required");
    expect(vi.mocked(api.connect)).not.toHaveBeenCalled();
    expect(vi.mocked(api.disconnect)).not.toHaveBeenCalled();
  });

  it("writes no password into the settings document", async () => {
    await renderPreferences();
    const host = document.querySelector<HTMLInputElement>(".prefs-field input") as HTMLInputElement;
    await changeField(host, "elsewhere.test");
    await vi.waitFor(() => expect(storedConnection().last_servers).toHaveLength(1));

    const documentText = JSON.stringify(state.stored).toLowerCase();
    expect(documentText).not.toContain("password");
    expect(documentText).not.toContain("secret");
    expect(document.querySelectorAll('input[type="password"]')).toHaveLength(0);
    expect([...document.querySelectorAll("button")].map((node) => node.textContent)).not.toContain(
      "Password",
    );
  });

  it("shows the identity as a read-only indicator", async () => {
    await renderPreferences();

    const indicator = document.querySelector('[data-testid="identity"]');
    expect(indicator?.textContent).toContain("11223344-55667788");
    expect(indicator?.querySelector("input")).toBeNull();
    expect(indicator?.querySelector("button")).toBeNull();
  });
});

describe("home palace and the launch offer", () => {
  async function setHomePalaceFromFields(palaceHost: string): Promise<void> {
    const host = document.querySelector<HTMLInputElement>(".prefs-field input") as HTMLInputElement;
    await changeField(host, palaceHost);
    // The commit that a field change starts also records the last server; wait
    // for it to finish (the home-palace button is disabled while it runs).
    await vi.waitFor(() =>
      expect(storedConnection().last_servers?.[0]?.host).toBe(palaceHost),
    );
    const button = buttonByText("Use the fields above as my home palace");
    await vi.waitFor(() => expect(button.disabled).toBe(false));
    button.click();
    await vi.waitFor(() => expect(storedConnection().home_palace?.host).toBe(palaceHost));
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="home-palace"]')?.textContent).toContain(
        palaceHost,
      ),
    );
  }

  it("persists the home palace and offers it on the next launch, without connecting", async () => {
    await renderPreferences();
    await setHomePalaceFromFields("home.test");

    // The change is connection-scoped: flagged, never applied silently.
    expect(document.querySelector(".prefs-reconnect")?.textContent).toContain("Reconnect required");
    expect(vi.mocked(api.connect)).not.toHaveBeenCalled();

    // The switch persists too.
    const box = document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]');
    expect(box?.disabled).toBe(false);
    box?.click();
    await vi.waitFor(() => expect(storedConnection().auto_connect).toBe(true));
    expect(document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]')?.checked).toBe(
      true,
    );

    // The next launch reads the same document and makes the offer.
    unmount(mounted as ReturnType<typeof mount>);
    mounted = undefined;
    document.body.innerHTML = "";
    mounted = mount(HomePalaceOffer, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(document.querySelector(".home-offer")).not.toBeNull());
    expect(document.querySelector(".home-offer")?.textContent).toContain("home.test:9998");

    buttonByText("Connect").click();
    await vi.waitFor(() =>
      expect(vi.mocked(api.connect)).toHaveBeenCalledWith(
        expect.objectContaining({ host: "home.test", port: 9998, username: "Guest" }),
      ),
    );
    await vi.waitFor(() => expect(document.querySelector(".home-offer")).toBeNull());
  });

  it("makes no offer without the switch, and lets the user refuse one", async () => {
    state.stored = {
      connection: { home_palace: server("home.test"), auto_connect: false },
    };
    mounted = mount(HomePalaceOffer, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(vi.mocked(api.getPrefs)).toHaveBeenCalled());
    expect(document.querySelector(".home-offer")).toBeNull();
    expect(vi.mocked(api.connect)).not.toHaveBeenCalled();

    // Same palace, switch on: the offer appears and "Not now" dismisses it.
    unmount(mounted);
    document.body.innerHTML = "";
    state.stored = {
      connection: { home_palace: server("home.test"), auto_connect: true },
    };

    mounted = undefined;
    mounted = mount(HomePalaceOffer, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(document.querySelector(".home-offer")).not.toBeNull());
    buttonByText("Not now").click();
    await tick();
    expect(document.querySelector(".home-offer")).toBeNull();
    expect(vi.mocked(api.connect)).not.toHaveBeenCalled();
  });

  it("offers nothing when auto-connect has no home palace to name", async () => {
    state.stored = { connection: { auto_connect: true } };
    mounted = mount(HomePalaceOffer, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(vi.mocked(api.getPrefs)).toHaveBeenCalled());
    expect(document.querySelector(".home-offer")).toBeNull();
  });

  it("stays hidden when the client is already connected to that palace", async () => {
    state.stored = { connection: { home_palace: server("home.test"), auto_connect: true } };
    store.status = "connected";
    store.settings = { ...store.settings, ...server("home.test") };

    mounted = mount(HomePalaceOffer, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(vi.mocked(api.getPrefs)).toHaveBeenCalled());
    expect(document.querySelector(".home-offer")).toBeNull();
  });

  it("puts the auto-connect box back when the write fails", async () => {
    state.stored = {
      connection: { home_palace: server("home.test"), auto_connect: false },
    };
    await renderPreferences();

    state.failNextSetPrefs = true;
    const box = document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]');
    expect(box?.checked).toBe(false);
    box?.click();

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("disk is full"),
    );
    expect(document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]')?.checked).toBe(
      false,
    );
    expect(storedConnection().auto_connect).toBe(false);
  });

  it("mounts the offer in the main shell, so the wiring cannot be forgotten", async () => {
    state.stored = { connection: { home_palace: server("home.test"), auto_connect: true } };
    mounted = mount(App, { target: document.body, props: { hash: "#/" } });
    await tick();

    await vi.waitFor(() => expect(document.querySelector(".home-offer")).not.toBeNull());
    expect(document.querySelector(".home-offer")?.textContent).toContain("home.test:9998");
    expect(document.querySelector("header.topbar")).not.toBeNull();
  });

  it("disables the auto-connect box until a home palace exists", async () => {
    await renderPreferences();

    const box = document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]');
    expect(box?.disabled).toBe(true);
    expect(describeServer(server("home.test"))).toBe("home.test:9998");

    await setHomePalaceFromFields("home.test");
    expect(
      document.querySelector<HTMLInputElement>('[data-testid="auto-connect"]')?.disabled,
    ).toBe(false);
  });
});
