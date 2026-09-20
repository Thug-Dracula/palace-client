import type { BagCollectionInfo, BagPropEntry, BagShelfInfo } from "./api";

/**
 * Pure view logic for the prop bag browser.
 *
 * Everything here is deliberately free of Svelte and Tauri so the filtering,
 * sorting, selection and windowing rules can be proven by unit tests. The
 * dialog component only wires these helpers to store state and DOM.
 *
 * The bag guarantee (no PalaceChat cache props) is enforced here as a defence
 * in depth: `filterBagEntries` drops anything whose `source` is not `"bag"`,
 * even though the backend catalog is already bag-only.
 */

export type BagFilter = "all" | "fav" | "trash";
export type BagSort = "name" | "id" | "collection";

export interface BagFilterState {
  filter: BagFilter;
  /** A collection name to restrict to, or `null` for every collection. */
  collection: string | null;
  query: string;
  sort: BagSort;
}

export const defaultBagFilterState = (): BagFilterState => ({
  filter: "all",
  collection: null,
  query: "",
  sort: "name",
});

/** One screenful of tiles. Keeps a 66k-record shelf from mounting 66k nodes. */
export const BAG_PAGE_SIZE = 240;

/** Stable identity for one catalog row: id alone is not unique. */
export function bagEntryKey(entry: BagPropEntry): string {
  return `${entry.collection}\u0000${entry.id}\u0000${entry.crc}`;
}

/** The human label for a row; falls back to the id when unnamed. */
export function bagEntryLabel(entry: BagPropEntry): string {
  const name = entry.name?.trim();
  return name && name !== "" ? name : `#${entry.id}`;
}

/** True only for entries the bag catalog is allowed to list. */
export function isBagSourced(entry: BagPropEntry): boolean {
  return entry.source === "bag";
}

/** Drop anything that is not bag-sourced. */
export function bagOnly(entries: readonly BagPropEntry[]): BagPropEntry[] {
  return entries.filter(isBagSourced);
}

/** A name / id / crc match, case-insensitive. */
export function matchesBagQuery(entry: BagPropEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") {
    return true;
  }
  const name = (entry.name ?? "").toLowerCase();
  return (
    name.includes(needle) ||
    String(entry.id).includes(needle) ||
    String(entry.crc).toLowerCase().includes(needle)
  );
}

/**
 * Apply the filter / collection / search rules and return a sorted copy.
 *
 * - `all` shows everything except trashed rows.
 * - `fav` shows favourites, also excluding trashed rows.
 * - `trash` shows only trashed rows.
 */
export function filterBagEntries(
  entries: readonly BagPropEntry[],
  state: BagFilterState,
): BagPropEntry[] {
  const filtered = bagOnly(entries).filter((entry) => {
    if (state.collection !== null && entry.collection !== state.collection) {
      return false;
    }
    if (state.filter === "trash") {
      if (!entry.trash) {
        return false;
      }
    } else {
      if (entry.trash) {
        return false;
      }
      if (state.filter === "fav" && !entry.fav) {
        return false;
      }
    }
    return matchesBagQuery(entry, state.query);
  });
  return sortBagEntries(filtered, state.sort);
}

function compareNamedFirst(a: BagPropEntry, b: BagPropEntry): number {
  const aNamed = bagEntryLabel(a).startsWith("#") === false;
  const bNamed = bagEntryLabel(b).startsWith("#") === false;
  if (aNamed !== bNamed) {
    return aNamed ? -1 : 1;
  }
  if (aNamed) {
    const byName = bagEntryLabel(a).localeCompare(bagEntryLabel(b));
    if (byName !== 0) {
      return byName;
    }
  }
  return a.id - b.id || a.crc - b.crc;
}

/** Sort a copy of the rows; ties break on id then crc so order is stable. */
export function sortBagEntries(
  entries: readonly BagPropEntry[],
  sort: BagSort,
): BagPropEntry[] {
  const sorted = [...entries];
  sorted.sort((a, b) => {
    if (sort === "id") {
      return a.id - b.id || a.crc - b.crc;
    }
    if (sort === "collection") {
      const byCollection = a.collection.localeCompare(b.collection);
      if (byCollection !== 0) {
        return byCollection;
      }
    }
    return compareNamedFirst(a, b);
  });
  return sorted;
}

/** How many browsable (non-trashed) bag rows each collection holds. */
export function collectionCounts(entries: readonly BagPropEntry[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const entry of bagOnly(entries)) {
    if (entry.trash) {
      continue;
    }
    counts.set(entry.collection, (counts.get(entry.collection) ?? 0) + 1);
  }
  return counts;
}

/** One row in the collection switcher. */
export interface BagCollectionView {
  name: string;
  path: string;
  writable: boolean;
  exists: boolean;
  count: number;
  health: BagShelfInfo | null;
}

/**
 * Merge the collection list, the shelf health list and the catalog counts into
 * the rows the switcher renders. My Bag is always first; shelves follow by name.
 */
export function buildCollectionView(
  collections: readonly BagCollectionInfo[],
  shelves: readonly BagShelfInfo[],
  entries: readonly BagPropEntry[],
): BagCollectionView[] {
  const counts = collectionCounts(entries);
  const health = new Map(shelves.map((shelf) => [shelf.name, shelf]));
  const byName = new Map<string, BagCollectionView>();

  for (const collection of collections) {
    byName.set(collection.name, {
      name: collection.name,
      path: collection.path,
      writable: collection.writable,
      exists: collection.exists,
      count: counts.get(collection.name) ?? 0,
      health: health.get(collection.name) ?? null,
    });
  }

  // A shelf can be discovered in the health list even when the collection list
  // is momentarily behind; never lose a browsable collection.
  for (const shelf of shelves) {
    if (!byName.has(shelf.name)) {
      byName.set(shelf.name, {
        name: shelf.name,
        path: shelf.path,
        writable: false,
        exists: true,
        count: counts.get(shelf.name) ?? 0,
        health: shelf,
      });
    }
  }

  const rows = [...byName.values()];
  rows.sort((a, b) => {
    const aMine = a.name === "My Bag" ? 0 : 1;
    const bMine = b.name === "My Bag" ? 0 : 1;
    return aMine - bMine || a.name.localeCompare(b.name);
  });
  return rows;
}

/** A shelf's health rendered for the switcher. */
export interface ShelfHealthView {
  /** Records the reader could not turn into a usable prop. */
  skipped: number;
  tone: "ok" | "warn" | "bad";
  detail: string;
}

export function shelfHealth(shelf: BagShelfInfo): ShelfHealthView {
  const skipped = shelf.undecodable + shelf.dropped;
  const tone: ShelfHealthView["tone"] =
    shelf.status === "unreadable" ? "bad" : shelf.status === "healthy" && skipped === 0 ? "ok" : "warn";
  const detail = `${shelf.records} records · ${shelf.named} named · ${skipped} skipped`;
  return { skipped, tone, detail };
}

/** One small attribute badge drawn on a tile. */
export interface PropBadge {
  label: string;
  title: string;
}

const PROP_BADGES: ReadonlyArray<{ bit: number; label: string; title: string }> = [
  { bit: 0x0004, label: "G", title: "Ghost" },
  { bit: 0x0010, label: "A", title: "Animate" },
  { bit: 0x0020, label: "B", title: "Bounce" },
  { bit: 0x0008, label: "R", title: "Rare" },
  { bit: 0x0002, label: "H", title: "Head" },
];

/** Decode the prop flag word into the badges an indicator list shows. */
export function propBadges(flags: number): PropBadge[] {
  return PROP_BADGES.filter((badge) => (flags & badge.bit) !== 0).map(({ label, title }) => ({
    label,
    title,
  }));
}

/** Return a new selection with `key` toggled. */
export function toggleSelection(selected: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(selected);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  return next;
}

/** The keys of every row in a list, for a select-all control. */
export function selectionKeys(entries: readonly BagPropEntry[]): string[] {
  return entries.map(bagEntryKey);
}

/** True when every row is currently worn. */
export function allWorn(entries: readonly BagPropEntry[], wornIds: readonly number[]): boolean {
  return entries.length > 0 && entries.every((entry) => wornIds.includes(entry.id));
}

/** At most `limit` rows are mounted, so thumbnail requests stay bounded. */
export function visibleWindow<T>(items: readonly T[], limit: number): T[] {
  return items.slice(0, Math.max(0, limit));
}

/** Grow the window without exceeding the total. */
export function nextLimit(current: number, total: number, step: number = BAG_PAGE_SIZE): number {
  return Math.min(total, Math.max(0, current) + Math.max(1, step));
}
