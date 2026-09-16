import type {
  ChatLine,
  ClientEvent,
  ConnectionStatus,
  RoomInfo,
  ScreenState,
  ServerBanner,
  Settings,
  UserInfo,
} from "./api";

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

const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;
const CHAT_LIMIT = 500;

class PalaceStore {
  status = $state<ConnectionStatus>("connecting");
  statusMessage = $state<string | null>(null);
  banner = $state<ServerBanner | null>(null);
  rooms = $state<RoomInfo[]>([]);
  users = $state<UserInfo[]>([]);
  room = $state<RoomInfo | null>(null);
  chat = $state<LocalChatLine[]>([]);
  screen = $state<ScreenState | null>(null);
  notes = $state<string[]>([]);
  notices = $state<string[]>([]);
  settings = $state<Settings>({ host: "localhost", port: 9998, username: "Guest" });
  roomFilter = $state("");

  zoom = $state(1);
  native = $state(false);

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

  setZoom(value: number): void {
    const clamped = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
    this.zoom = Math.round(clamped * 100) / 100;
  }

  nudgeZoom(delta: number): void {
    this.setZoom(this.zoom + delta);
  }

  apply(event: ClientEvent): void {
    switch (event.type) {
      case "status":
        this.status = event.status;
        this.statusMessage = event.message;
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
        break;
      case "chat":
        this.pushLine(event.line);
        break;
      case "screen":
        this.screen = event.screen;
        this.notes = event.screen.notes;
        if (!event.screen.geometry.native) {
          this.zoom = event.screen.geometry.zoom;
        }
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
