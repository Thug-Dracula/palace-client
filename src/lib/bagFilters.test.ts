import { describe, expect, it } from "vitest";

import type { BagCollectionInfo, BagPropEntry, BagShelfInfo } from "./api";
import {
  BAG_PAGE_SIZE,
  allWorn,
  bagEntryKey,
  bagEntryLabel,
  buildCollectionView,
  collectionCounts,
  defaultBagFilterState,
  filterBagEntries,
  nextLimit,
  propBadges,
  selectionKeys,
  shelfHealth,
  sortBagEntries,
  toggleSelection,
  visibleWindow,
} from "./bagFilters";

const entry = (over: Partial<BagPropEntry> = {}): BagPropEntry => ({
  id: 1,
  crc: 100,
  name: null,
  w: 44,
  h: 44,
  flags: 0,
  fav: false,
  trash: false,
  collection: "My Bag",
  source: "bag",
  ...over,
});

const state = (over: Partial<ReturnType<typeof defaultBagFilterState>> = {}) => ({
  ...defaultBagFilterState(),
  ...over,
});

describe("filterBagEntries", () => {
  const entries = [
    entry({ id: 1, crc: 11, name: "Sword", collection: "My Bag" }),
    entry({ id: 2, crc: 22, name: "Shield", collection: "My Bag", fav: true }),
    entry({ id: 3, crc: 33, name: "Trashed", collection: "My Bag", trash: true }),
    entry({ id: 4, crc: 44, name: "Lantern", collection: "Medieval", fav: true }),
    entry({ id: 5, crc: 55, name: null, collection: "Medieval", trash: true }),
  ];

  it("All shows every non-trashed row and hides trash", () => {
    const ids = filterBagEntries(entries, state({ filter: "all" })).map((e) => e.id);
    expect(ids).toEqual([4, 2, 1]); // name sort: Lantern, Shield, Sword
    expect(ids).not.toContain(3);
    expect(ids).not.toContain(5);
  });

  it("Favourites shows only favourited, non-trashed rows", () => {
    const ids = filterBagEntries(entries, state({ filter: "fav" })).map((e) => e.id);
    expect(ids).toEqual([4, 2]);
  });

  it("Trash shows only trashed rows", () => {
    const ids = filterBagEntries(entries, state({ filter: "trash" })).map((e) => e.id);
    expect(ids).toEqual([3, 5]);
  });

  it("restricts to one collection", () => {
    const ids = filterBagEntries(entries, state({ collection: "Medieval" })).map((e) => e.id);
    expect(ids).toEqual([4]);
  });

  it("searches name, id and crc, case-insensitively", () => {
    expect(filterBagEntries(entries, state({ query: "shie" })).map((e) => e.id)).toEqual([2]);
    expect(filterBagEntries(entries, state({ query: "4" })).map((e) => e.id)).toEqual([4]);
    expect(filterBagEntries(entries, state({ query: "44" })).map((e) => e.id)).toEqual([4]);
  });

  it("never returns a cache-sourced row, even when one is injected", () => {
    const injected = [
      ...entries,
      { ...entry({ id: 99, name: "CacheOnly" }), source: "cache" as unknown as "bag" },
    ];
    for (const filter of ["all", "fav", "trash"] as const) {
      const ids = filterBagEntries(injected, state({ filter })).map((e) => e.id);
      expect(ids).not.toContain(99);
    }
  });

  it("combines filter, collection and query", () => {
    const ids = filterBagEntries(
      entries,
      state({ filter: "fav", collection: "My Bag" }),
    ).map((e) => e.id);
    expect(ids).toEqual([2]);
  });
});

describe("sortBagEntries", () => {
  const entries = [
    entry({ id: 30, crc: 1, name: "Zeta", collection: "B" }),
    entry({ id: 10, crc: 2, name: "Alpha", collection: "C" }),
    entry({ id: 20, crc: 3, name: null, collection: "A" }),
  ];

  it("sorts named rows first, then unnamed rows by id", () => {
    expect(sortBagEntries(entries, "name").map((e) => e.id)).toEqual([10, 30, 20]);
  });

  it("sorts numerically by id", () => {
    expect(sortBagEntries(entries, "id").map((e) => e.id)).toEqual([10, 20, 30]);
  });

  it("sorts by collection then label", () => {
    expect(sortBagEntries(entries, "collection").map((e) => e.collection)).toEqual([
      "A",
      "B",
      "C",
    ]);
  });
});

describe("collection helpers", () => {
  it("counts only non-trashed bag rows", () => {
    const counts = collectionCounts([
      entry({ id: 1, collection: "My Bag" }),
      entry({ id: 2, collection: "My Bag", trash: true }),
      entry({ id: 3, collection: "Shelf" }),
    ]);
    expect(counts.get("My Bag")).toBe(1);
    expect(counts.get("Shelf")).toBe(1);
  });

  it("merges collections, shelf health and counts with My Bag first", () => {
    const collections: BagCollectionInfo[] = [
      { path: "/bag/My Bag.prp", name: "My Bag", writable: true, exists: true },
      { path: "/bag/shelves/Server.prp", name: "Server", writable: false, exists: true },
    ];
    const shelves: BagShelfInfo[] = [
      {
        name: "Server",
        path: "/bag/shelves/Server.prp",
        status: "degraded",
        records: 66000,
        named: 65000,
        undecodable: 1000,
        dropped: 0,
        error: null,
      },
    ];
    const rows = buildCollectionView(collections, shelves, [
      entry({ id: 1, collection: "My Bag" }),
      entry({ id: 2, collection: "Server" }),
      entry({ id: 3, collection: "Server" }),
    ]);
    expect(rows.map((row) => row.name)).toEqual(["My Bag", "Server"]);
    expect(rows[0]).toMatchObject({ writable: true, count: 1, health: null });
    expect(rows[1]).toMatchObject({ writable: false, count: 2 });
    expect(rows[1].health?.records).toBe(66000);
  });

  it("keeps a shelf that is missing from the collection list", () => {
    const rows = buildCollectionView(
      [],
      [
        {
          name: "OnlyShelf",
          path: "/x.prp",
          status: "healthy",
          records: 1,
          named: 1,
          undecodable: 0,
          dropped: 0,
          error: null,
        },
      ],
      [entry({ id: 1, collection: "OnlyShelf" })],
    );
    expect(rows.map((row) => row.name)).toEqual(["OnlyShelf"]);
  });

  it("reports shelf health, counting undecodable plus dropped as skipped", () => {
    const health = shelfHealth({
      name: "S",
      path: "/s.prp",
      status: "degraded",
      records: 10,
      named: 8,
      undecodable: 1,
      dropped: 2,
      error: null,
    });
    expect(health.skipped).toBe(3);
    expect(health.tone).toBe("warn");
    expect(health.detail).toBe("10 records · 8 named · 3 skipped");
  });

  it("marks a healthy shelf ok and an unreadable shelf bad", () => {
    const healthy = shelfHealth({
      name: "S",
      path: "/s.prp",
      status: "healthy",
      records: 1,
      named: 1,
      undecodable: 0,
      dropped: 0,
      error: null,
    });
    const bad = shelfHealth({
      name: "U",
      path: "/u.prp",
      status: "unreadable",
      records: 0,
      named: 0,
      undecodable: 0,
      dropped: 0,
      error: "boom",
    });
    expect(healthy.tone).toBe("ok");
    expect(bad.tone).toBe("bad");
  });
});

describe("propBadges", () => {
  it("is empty for an unremarkable prop", () => {
    expect(propBadges(0)).toEqual([]);
  });

  it("decodes ghost, animate, bounce, rare and head", () => {
    expect(propBadges(0x0004 | 0x0010 | 0x0020 | 0x0008 | 0x0002).map((b) => b.label)).toEqual([
      "G",
      "A",
      "B",
      "R",
      "H",
    ]);
  });
});

describe("selection and windowing", () => {
  it("toggles a key in a fresh set", () => {
    const start = new Set(["a"]);
    const added = toggleSelection(start, "b");
    expect([...added].sort()).toEqual(["a", "b"]);
    expect(start.has("b")).toBe(false); // original untouched
    expect([...toggleSelection(added, "b")].sort()).toEqual(["a"]);
  });

  it("keys rows by collection, id and crc", () => {
    expect(bagEntryKey(entry({ collection: "My Bag", id: 7, crc: 9 }))).toBe("My Bag\u00007\u00009");
    expect(selectionKeys([entry({ id: 1 }), entry({ id: 2 })])).toHaveLength(2);
  });

  it("detects an all-worn selection", () => {
    const rows = [entry({ id: 1 }), entry({ id: 2 })];
    expect(allWorn(rows, [1, 2])).toBe(true);
    expect(allWorn(rows, [1])).toBe(false);
    expect(allWorn([], [])).toBe(false);
  });

  it("bounds the mounted window and grows it without exceeding the total", () => {
    const rows = Array.from({ length: 1000 }, (_, index) => index);
    expect(visibleWindow(rows, 10)).toHaveLength(10);
    expect(visibleWindow(rows, 10)[0]).toBe(0);
    expect(nextLimit(10, 1000)).toBe(10 + BAG_PAGE_SIZE);
    expect(nextLimit(990, 1000)).toBe(1000);
  });

  it("never mounts more rows than exist for a small list", () => {
    expect(visibleWindow([1, 2, 3], BAG_PAGE_SIZE)).toEqual([1, 2, 3]);
    expect(nextLimit(0, 3)).toBe(3);
  });
});

describe("bagEntryLabel", () => {
  it("uses the name, falling back to the id", () => {
    expect(bagEntryLabel(entry({ name: " Sword " }))).toBe("Sword");
    expect(bagEntryLabel(entry({ name: null }))).toBe("#1");
    expect(bagEntryLabel(entry({ name: "   " }))).toBe("#1");
  });
});
