import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const EVENT_NAME = "palace://event";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

export interface Settings {
  host: string;
  port: number;
  username: string;
  soundfont: string | null;
}

export interface AudioState {
  enabled: boolean;
  volume: number;
  soundfont: string | null;
  soundfont_exists: boolean;
}

export interface ServerBanner {
  host: string;
  port: number;
  byte_order: string;
  user_id: number;
  version: string | null;
  name: string | null;
  media_base: string | null;
  total_users: number | null;
}

export interface RoomInfo {
  id: number;
  name: string;
  users: number;
  flags: number;
}

export interface UserInfo {
  id: number;
  name: string;
  face: number;
  color: number;
  room_id: number;
  x: number;
  y: number;
  props: number[];
  away: boolean;
  is_self: boolean;
}

export type ChatKind = "talk" | "whisper" | "system" | "error";

export interface ChatLine {
  seq: number;
  user_id: number;
  name: string;
  text: string;
  kind: ChatKind;
}

export interface ViewGeometry {
  room_w: number;
  room_h: number;
  viewport_w: number;
  viewport_h: number;
  dpr: number;
  zoom: number;
  native: boolean;
  scale: number;
  content_x: number;
  content_y: number;
  content_w: number;
  content_h: number;
  bitmap_w: number;
  bitmap_h: number;
}

export interface ScreenState {
  version: number;
  room_id: number;
  room_name: string;
  avatars: number;
  loose_props: number;
  props_pending: number;
  notes: string[];
  geometry: ViewGeometry;
}

export interface ScriptReport {
  event: string;
  fired: number;
  effects: string[];
  problems: string[];
}

export type ClientEvent =
  | { type: "status"; status: ConnectionStatus; message: string | null }
  | { type: "banner"; banner: ServerBanner }
  | { type: "rooms"; rooms: RoomInfo[] }
  | { type: "users"; users: UserInfo[] }
  | { type: "room_entered"; room: RoomInfo }
  | { type: "chat"; line: ChatLine }
  | { type: "screen"; screen: ScreenState }
  | { type: "tooltip"; text: string | null }
  | {
      type: "script";
      event: string;
      fired: number;
      effects: string[];
      problems: string[];
    }
  | { type: "note"; text: string }
  | { type: "sound"; name: string }
  | { type: "midi_play"; name: string }
  | { type: "midi_loop"; name: string; loops: number }
  | { type: "midi_stop" }
  | { type: "beep" };

export const frameUrl = (version: number): string => `palace://localhost/frame?v=${version}`;

export const facesUrl = (): string => "palace://localhost/faces";

export interface FaceGrid {
  cell: number;
  faces: number;
  colors: number;
  rows: number[][];
}

export const facesGrid = async (): Promise<FaceGrid> => {
  const response = await fetch("palace://localhost/faces.json");
  if (!response.ok) {
    throw new Error(`face grid: HTTP ${response.status}`);
  }
  return (await response.json()) as FaceGrid;
};

export const getSettings = (): Promise<Settings> => invoke("get_settings");

export const connect = (settings: Settings): Promise<void> =>
  invoke("connect", {
    host: settings.host,
    port: settings.port,
    username: settings.username,
  });

export const disconnect = (): Promise<void> => invoke("disconnect");

export const gotoRoom = (roomId: number): Promise<void> => invoke("goto_room", { roomId });

export const say = (text: string): Promise<void> => invoke("say", { text });

export const setViewport = (
  width: number,
  height: number,
  dpr: number,
  zoom: number,
  native: boolean,
): Promise<void> => invoke("set_viewport", { width, height, dpr, zoom, native });

export const setUiScale = (scale: number): Promise<number> => invoke("set_ui_scale", { scale });

export const refresh = (): Promise<void> => invoke("refresh");

export const click = (x: number, y: number): Promise<void> => invoke("click", { x, y });

export const mousemove = (x: number, y: number): Promise<void> => invoke("mousemove", { x, y });

export const mouseLeave = (): Promise<void> => invoke("mouse_leave");

export const setVisibility = (names: boolean, avatars: boolean): Promise<void> =>
  invoke("set_visibility", { names, avatars });

export const setAvatar = (face: number, color: number): Promise<void> =>
  invoke("set_avatar", { face, color });

export const setProps = (props: number[]): Promise<void> => invoke("set_props", { props });

export const getAudioState = (): Promise<AudioState> => invoke("get_audio_state");

export const setSoundfont = (path: string | null): Promise<void> =>
  invoke("set_soundfont", { path });

export const setAudioEnabled = (enabled: boolean): Promise<void> =>
  invoke("set_audio_enabled", { enabled });

export const setVolume = (volume: number): Promise<number> => invoke("set_volume", { volume });

export const onEvent = (handler: (event: ClientEvent) => void): Promise<UnlistenFn> =>
  listen<ClientEvent>(EVENT_NAME, (message) => handler(message.payload));
