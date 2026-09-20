import type {
  AudioState,
  AvatarRoster,
  BagCollectionInfo,
  BagKey,
  BagOutfit,
  BagPropEntry,
  BagShelfInfo,
  BagTrashEntry,
  ChatLine,
  ClientEvent,
  ConnectionStatus,
  GatherResult,
  RoomInfo,
  ScreenState,
  ServerBanner,
  Settings,
  UserInfo,
} from "./api";
import * as api from "./api";

const FACE_COLORS = [
  "#ff6b6b",
  "#ffb454",
  "#7fd962",
  "#5ccfe6",
  "#c099ff",
  "#ff8fd0",
  "#ddd76b",
  "#8fa3b8",
];

export const faceColor = (index: number): string => {
  const n = FACE_COLORS.length;
  return FACE_COLORS[((index % n) + n) % n];
};

export interface LocalChatLine extends ChatLine {
  pending?: boolean;
}

export interface TogglePropResult {
  ok: boolean;
  error?: string;
}

const MIN_SCALE = 0.5;
const MAX_SCALE = 3;
const CHAT_LIMIT = 500;

export const MAX_PROPS = 9;

class PalaceStore {
  status = $state<ConnectionStatus>("connecting");
  statusMessage = $state<string | null>(null);
  banner = $state<ServerBanner | null>(null);
  rooms = $state<RoomInfo[]>([]);
  users = $state<UserInfo[]>([]);
  room = $state<RoomInfo | null>(null);
  chat = $state<LocalChatLine[]>([]);
  screen = $state<ScreenState | null>(null);
  avatars = $state<AvatarRoster | null>(null);
  tooltip = $state<string | null>(null);
  notes = $state<string[]>([]);
  notices = $state<string[]>([]);
  settings = $state<Settings>({ host: "localhost", port: 9998, username: "Guest", soundfont: null });
  audio = $state<AudioState>({
    enabled: true,
    volume: 1,
    soundfont: null,
    soundfont_exists: false,
  });
  roomFilter = $state("");

  bagCollections = $state<BagCollectionInfo[]>([]);
  bagShelves = $state<BagShelfInfo[]>([]);
  bagCatalog = $state<BagPropEntry[]>([]);
  bagTrash = $state<BagTrashEntry[]>([]);
  outfits = $state<BagOutfit[]>([]);
  bagError = $state<string | null>(null);
  bagLoaded = $state(false);

  scale = $state(1);
  native = $state(false);
  showNames = $state(true);
  showAvatars = $state(true);

  viewport = { width: 960, height: 540, dpr: 1 };

  private localSeq = -1;

  get filteredRooms(): RoomInfo[] {
    const query = this.roomFilter.trim().toLowerCase();
    if (!query) {
      return this.rooms;
    }
    return this.rooms.filter((room) => room.name.toLowerCase().includes(query));
  }

  get connected(): boolean {
    return this.status === "connected";
  }

  get self(): UserInfo | null {
    return this.users.find((user) => user.is_self) ?? null;
  }

  setVisibility(names: boolean, avatars: boolean): void {
    this.showNames = names;
    this.showAvatars = avatars;
    void api.setVisibility(names, avatars).catch(() => {});
  }

  setProps(props: number[]): void {
    void api.setProps(props).catch(() => {});
  }

  toggleProp(id: number): TogglePropResult {
    const self = this.self;
    if (!self) {
      return { ok: false, error: "You are not in a room yet, so there is nothing to wear." };
    }
    const worn = self.props.includes(id);
    if (worn) {
      const next = self.props.filter((prop) => prop !== id);
      self.props = next;
      this.setProps(next);
      return { ok: true };
    }
    if (self.props.length >= MAX_PROPS) {
      return {
        ok: false,
        error: `You can wear at most ${MAX_PROPS} props at once — take one off first.`,
      };
    }
    const next = [...self.props, id];
    self.props = next;
    this.setProps(next);
    return { ok: true };
  }

  takeOffAvatar(): void {
    const self = this.self;
    if (self) {
      self.props = [];
    }
    this.setProps([]);
  }

  /**
   * Wear or take off one bag entry.
   *
   * The bag identity is `(id, crc)`: the crc selects which variant the bag
   * showed, while the protocol wears props by id, so `set_props` carries the
   * id. This reuses the existing `toggleProp` path, so `MAX_PROPS` and
   * `takeOffAvatar` are unchanged.
   */
  toggleBagProp(entry: BagPropEntry): TogglePropResult {
    return this.toggleProp(entry.id);
  }

  /** Wear every entry, stopping at the first refusal (e.g. the 9-prop limit). */
  wearBagEntries(entries: BagPropEntry[]): TogglePropResult {
    for (const entry of entries) {
      const result = this.toggleProp(entry.id);
      if (!result.ok) {
        return result;
      }
    }
    return { ok: true };
  }

  /** Replace the worn set with an outfit, through the same `set_props` path. */
  async applyOutfit(name: string): Promise<TogglePropResult> {
    const self = this.self;
    if (!self) {
      return { ok: false, error: "You are not in a room yet, so there is nothing to wear." };
    }
    const application = await api.outfitsApply(name);
    if (application.worn.length > MAX_PROPS) {
      return {
        ok: false,
        error: `That outfit has ${application.worn.length} props; you can wear at most ${MAX_PROPS}.`,
      };
    }
    const next = application.worn.map((key) => key.id);
    self.props = next;
    this.setProps(next);
    return { ok: true };
  }

  /** Reload every bag surface from the backend. */
  async loadBag(): Promise<void> {
    try {
      const [collections, shelves, catalog, trash, outfits] = await Promise.all([
        api.bagCollections(),
        api.bagShelves(),
        api.bagCatalog(),
        api.bagTrashList(),
        api.outfitsList(),
      ]);
      this.bagCollections = collections;
      this.bagShelves = shelves;
      this.bagCatalog = catalog;
      this.bagTrash = trash;
      this.outfits = outfits;
      this.bagError = null;
    } catch (error) {
      this.bagError = error instanceof Error ? error.message : String(error);
    } finally {
      this.bagLoaded = true;
    }
  }

  /** Gather a room prop into My Bag; reloads the catalog when it landed. */
  async gatherProp(id: number, crc: number | null = null): Promise<GatherResult> {
    const result = await api.gatherProp(id, crc);
    if (result.outcome === "added") {
      await this.loadBag();
    }
    return result;
  }

  async setBagFavourite(entry: BagPropEntry, favourite: boolean): Promise<void> {
    await api.bagFavourite(entry.id, entry.crc, favourite);
    await this.loadBag();
  }

  async trashBagProp(entry: BagPropEntry): Promise<void> {
    await api.bagTrash(entry.id, entry.crc);
    await this.loadBag();
  }

  async restoreBagTrash(entry: BagTrashEntry): Promise<void> {
    await api.bagTrashRestore(entry.id, entry.crc);
    await this.loadBag();
  }

  async purgeBagTrash(): Promise<void> {
    await api.bagTrashPurge();
    await this.loadBag();
  }

  /** Save the currently worn props as an outfit, resolving each id's crc. */
  async saveOutfit(name: string): Promise<void> {
    const self = this.self;
    const worn: BagKey[] = [];
    if (self) {
      for (const id of self.props) {
        const entry = this.bagCatalog.find((candidate) => candidate.id === id);
        if (entry) {
          worn.push({ id: entry.id, crc: entry.crc });
        }
      }
    }
    this.outfits = await api.outfitsSave(name, worn);
  }

  async renameOutfit(from: string, to: string): Promise<void> {
    this.outfits = await api.outfitsRename(from, to);
  }

  async deleteOutfit(name: string): Promise<void> {
    this.outfits = await api.outfitsDelete(name);
  }

  async duplicateOutfit(source: string, target: string): Promise<void> {
    this.outfits = await api.outfitsDuplicate(source, target);
  }

  setScale(value: number): void {
    const clamped = Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
    this.scale = Math.round(clamped * 100) / 100;
    void api
      .setUiScale(this.scale)
      .then((applied) => {
        this.scale = applied;
      })
      .catch(() => {});
  }

  async loadAudio(): Promise<void> {
    this.audio = await api.getAudioState();
  }

  async chooseSoundfont(path: string | null): Promise<void> {
    await api.setSoundfont(path);
    this.audio = await api.getAudioState();
  }

  async setAudioEnabled(enabled: boolean): Promise<void> {
    await api.setAudioEnabled(enabled);
    this.audio = await api.getAudioState();
  }

  async setAudioVolume(volume: number): Promise<number> {
    const applied = await api.setVolume(volume);
    this.audio = await api.getAudioState();
    return applied;
  }

  apply(event: ClientEvent): void {
    switch (event.type) {
      case "status":
        this.status = event.status;
        this.statusMessage = event.message;
        // A roster is only meaningful while a session is live; drop it on the
        // way out so a reconnect cannot paint stale sprites over a new room.
        if (event.status === "disconnected" || event.status === "error") {
          this.avatars = null;
        }
        break;
      case "banner":
        this.banner = event.banner;
        break;
      case "rooms":
        this.rooms = [...event.rooms].sort((a, b) => a.name.localeCompare(b.name));
        break;
      case "users":
        this.users = event.users;
        break;
      case "room_entered":
        this.room = event.room;
        this.avatars = null;
        break;
      case "chat":
        this.pushLine(event.line);
        break;
      case "screen":
        this.screen = event.screen;
        this.notes = event.screen.notes;
        break;
      case "avatars":
        this.avatars = event.roster;
        break;
      case "tooltip":
        this.tooltip = event.text;
        break;
      case "script": {
        const summary = `ON ${event.event}: ${event.fired} handler(s)`;
        this.notices = [
          ...this.notices,
          ...event.effects.slice(0, 3),
          ...event.problems,
          event.effects.length > 3 ? `${summary} (+${event.effects.length - 3} more)` : summary,
        ].slice(-6);
        break;
      }
      case "note":
        this.notices = [...this.notices, event.text].slice(-6);
        break;
    }
  }

  pushLine(line: ChatLine): void {
    if (line.kind === "talk" && line.user_id === this.banner?.user_id) {
      const index = this.chat.findIndex(
        (candidate) => candidate.pending && candidate.text === line.text,
      );
      if (index >= 0) {
        this.chat.splice(index, 1);
      }
    }
    this.chat.push(line);
    if (this.chat.length > CHAT_LIMIT) {
      this.chat.splice(0, this.chat.length - CHAT_LIMIT);
    }
  }

  echoLocal(text: string): void {
    const line: LocalChatLine = {
      seq: this.localSeq--,
      user_id: this.banner?.user_id ?? -1,
      name: this.settings.username,
      text,
      kind: "talk",
      pending: true,
    };
    this.chat.push(line);
    if (this.chat.length > CHAT_LIMIT) {
      this.chat.splice(0, this.chat.length - CHAT_LIMIT);
    }
  }
}

export const store = new PalaceStore();
