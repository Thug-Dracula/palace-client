# Palace protocol reference

This is what this client actually implements on the wire: how frames are framed,
how byte order is handled, the logon exchange, the full layout of the
room-description message, and every opcode the project has observed or
identified.

It is deliberately detailed. The [README](README.md) explains what the client is
and how to run it; this document is for anyone who needs to speak the protocol,
compare notes with another implementation, or understand why a decoder is shaped
the way it is.

Everything here was derived by probing live servers and by reading the reference
implementations named in the README's provenance section. Where a field's meaning
is still uncertain, this document says so rather than guessing.

---

## Wire format and logon

### Framing

Every message, both directions, for the whole life of the connection:

```text
struct ClientMsg {
    uint32 eventType;   // 4-char ASCII mnemonic, written in session byte order
    uint32 length;      // body length
    sint32 refNum;      // operand; meaning depends on the opcode
    uint8  msg[length];
};
```

There is no terminator and no checksum, so `length` must be trusted exactly.
`palace-wire` refuses payloads larger than 50 MB rather than allocating blindly.

### Endianness (session-fatal, handled once)

The server speaks first with `MSG_TIYID`, a zero-length frame whose first four
bytes reveal the byte order:

| bytes on the wire | server writes ints as | client action |
|---|---|---|
| `ryit` | little-endian | use [`ByteOrder::Little`] |
| `tiyr` | big-endian | use [`ByteOrder::Big`] |
| `pser` | HTTP tunnel | **exit with a clear error** (unsupported) |

`ryit`/`tiyr` are the same 32-bit value (`0x74697972`) written in the server's
native order; the protocol reference calls the little-endian form `MSG_DIYIT`.
Detection happens once. `Reader`/`Writer` carry the resulting `ByteOrder`, and
no call site performs a conditional swap.

### Logon

`MSG_LOGON` (`regi`), `refNum` 0, body = 128-byte `AuxRegistrationRec`:

| offset | size | field |
|---|---|---|
| 0 | 4 | `crc` |
| 4 | 4 | `counter` |
| 8 | 32 | `userName` (`Str31`) |
| 40 | 32 | `wizPassword` (`Str31`) |
| 72 | 4 | `auxFlags` |
| 76 | 4 | `puidCtr` |
| 80 | 4 | `puidCRC` |
| 84 | 4 | `demoElapsed` |
| 88 | 4 | `totalElapsed` |
| 92 | 4 | `demoLimit` |
| 96 | 2 | `desiredRoom` |
| 98 | 6 | `reserved` |
| 104 | 4 | `ulRequestedProtocolVersion` |
| 108 | 4 | `ulUploadCaps` |
| 112 | 4 | `ulDownloadCaps` |
| 116 | 4 | `ul2DEngineCaps` |
| 120 | 4 | `ul2DGraphicsCaps` |
| 124 | 4 | `ul3DEngineCaps` |

The probe **mirrors `palace_walker.py`'s logon byte for byte** (a unit test
asserts the full 140-byte packet). That means the deliberately misstated
capability values and the reserved client tag are the reference client's, not
honest ones:

| field | value | note |
|---|---|---|
| `crc` / `counter` | `0x32fb23e9` / `0xaa18198f` | guest registration |
| `auxFlags` | `0x80000008` | `Authenticate` + OS tag 8 |
| `puidCtr` / `puidCRC` | `0xe0546e4f` / `0x92c96d66` | |
| `demo*` | 72000 | historical, unused |
| `reserved` | `SCOUT1` | see note below |
| protocol version | `0x0007abcc` | server ignores it |
| `ulUploadCaps` | `0x41` | `ASSETS_PALACE \| FILES_HTTP` |
| `ulDownloadCaps` | `0x151` | `ASSETS_PALACE \| FILES_PALACE \| FILES_HTTP \| FILES_HTTPSRVR` |
| `ul2DEngineCaps` / `ul2DGraphicsCaps` | 1 / 1 | |
| `ul3DEngineCaps` | 0 | |

> **Note on `reserved`.** The task brief called this field `"OPNPAL"`, which is
> what OpenPalace writes; Taj writes `"example-host-1"` and the captured PalaceChat 5
> client wrote `"example-host-2"`. `palace_walker.py` overwrites it with `"SCOUT1"`, and
> the brief also says to mirror the walker exactly — so `SCOUT1` is the default
> here, verified working against the live server. The field is logged by the
> server and not validated; `ReferenceProfile` exposes it if a different tag is
> ever wanted.

### Logon burst, keepalive, exit

After `regi` the server sends a burst. Observed live (9 frames):

`vers` → `sinf` → `uSta` → `log ` → `HTTP` → `room` → `rprs` → `endr` → `nprs`

(`rep2` is optional: it only appears when the server runs in
"guests-are-members" mode.) Keepalive is `ping` → `pong`, answered
automatically; the probe also sends `ping` if needed. Clean exit is `bye `.

Room and user lists are fetched explicitly with `rLst` and `uLst` (empty body,
`refNum` 0). The response carries the count in `refNum` and the records in the
body.

| message | record layout |
|---|---|
| `rLst` response | `sint32 roomID`, `sint16 flags`, `sint16 nbrUsers`, `PString name` padded so the field length is a multiple of 4 |
| `uLst` / `rprs` | `UserID`, `sint16 flags`, `RoomID`, `PString name` padded to a multiple of 4 |
| `nprs` / `rprs` entries | `UserRec`, 124 bytes |

> The live pserver pads aligned `PString`s with **uninitialised bytes** (observed
> `00 6c 65` after `"Rico"`). The decoder skips the padding instead of requiring
> zeros, and there is a test for it.

### Tolerant parsing

Unknown opcodes are values, not errors. `Message::decode` returns
`Message::Unknown { .. }`; the probe logs the opcode in hex plus a payload
preview and continues. A malformed *known* message is reported and skipped
rather than aborting the session. Framing errors are fatal, because the stream
can no longer be trusted.

---

## RoomDesc: the room-description message

`MSG_ROOMDESC` (`room`) is the hardest message in the protocol. It is a flat
40-byte header — `RoomRec` — whose fields are **byte offsets into a trailing
variable-length buffer** (`varBuf`). Nothing beyond the header is reached
directly, so every sub-structure is an indirect, bounds-checked read.
`crates/palace-room/` implements the walker; this section is the layout it
established, field by field.

### Why a separate crate

`palace-room` is its own crate rather than an extension of `palace-wire`.
`palace-wire` is deliberately a dependency-free framing/opcode layer whose
`RoomDescription` only decodes the header and four names; this milestone adds a
large model builder (overlays, hotspots, states, loose props, draw commands) on
top. Keeping it separate matches the workspace table in
`$CORPUS/TAURI-CLIENT-SCOPE.md` (`palace-room` depends on `palace-wire`) and
leaves `palace-wire`'s tests and public API untouched.

### The `RoomRec` header (40 bytes)

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 4 | `roomFlags` | `RF_*` room-attribute bits |
| 4 | 4 | `facesID` | default avatar face-set id |
| 8 | 2 | `roomID` | room id |
| 10 | 2 | `roomNameOfst` | index into `varBuf` of the room-name `PString` (`0` = absent) |
| 12 | 2 | `pictNameOfst` | index of the background-picture file name |
| 14 | 2 | `artistNameOfst` | index of the artist name |
| 16 | 2 | `passwordOfst` | index of the room password |
| 18 | 2 | `nbrHotspots` | number of hotspots |
| 20 | 2 | `hotspotOfst` | index of the **hotspot array** (48-byte entries) |
| 22 | 2 | `nbrPictures` | number of image overlays |
| 24 | 2 | `pictureOfst` | index of the **overlay array** (12-byte entries) |
| 26 | 2 | `nbrDrawCmds` | number of draw commands |
| 28 | 2 | `firstDrawCmd` | index of the first draw record (**linked list**) |
| 30 | 2 | `nbrPeople` | users currently in the room |
| 32 | 2 | `nbrLProps` | number of loose props |
| 34 | 2 | `firstLProp` | index of the first loose prop (**linked list**) |
| 36 | 2 | `reserved` | alignment filler; `0` |
| 38 | 2 | `lenVars` | length of `varBuf` |
| 40 | `lenVars` | `varBuf` | the variable data all offsets point into |

All integers are session-endian (see *Endianness* above). There is **no width or
height** in the record: room size comes from the background image.

### Trailing padding

The reference client (OpenPalace `handleReceiveRoomDescription`) computes the
bytes after `varBuf` as:

```text
padding = payload_len - lenVars - 40
```

and skips them. Verified against the corpus: every one of the 795
single-record payloads has exactly **4** trailing bytes, and `RoomDesc::trailing_len`
exposes the value. The formula is **not** used to locate anything — records are
delimited by `lenVars`, so a server that pads differently does not break the
parse.

### Strings (`PString`, `CString`)

Inside a room description a `PString` is one length byte followed by that many
bytes, decoded as Latin-1. Unlike the `rLst`/`uLst` names, room `PString`s are
**not** padded to a 4-byte boundary — the next string starts immediately after:

* Balamb Garden: `roomNameOfst = 2`, `"Balamb Garden"` is 13 bytes, and
  `pictNameOfst = 16` exactly (`2 + 1 + 13`).
* `1003.bin`: room name `"M6"` at offset 6, `pictNameOfst = 9` (`6 + 1 + 2`).

Hotspot scripts are NUL-terminated `CString`s. `0` means "absent" for every
string offset.

### Image overlay / `PictureRec` (12 bytes each)

Array at `pictureOfst`, `nbrPictures` entries, stride 12.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `refCon` |
| 4 | 2 | `picID` |
| 6 | 2 | `picNameOfst` (offset of the file-name `PString`) |
| 8 | 2 | `transColor` — transparency index |
| 10 | 2 | `reserved` |

`transColor`: `-1` = no transparency, `0` = use the bottom-left pixel,
`> 0` = palette index (OpenPalace `PalaceImageOverlay`).

### Hotspot (48 bytes each)

Array at `hotspotOfst`, `nbrHotspots` entries, stride 48.

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 4 | `scriptEventMask` | `PE_*` event bits |
| 4 | 4 | `flags` | `HS_*` display/behaviour bits |
| 8 | 4 | `secureInfo` | documented unused |
| 12 | 4 | `refCon` | documented unused |
| 16 | 2 | `loc.y` | absolute pixel |
| 18 | 2 | `loc.x` | absolute pixel |
| 20 | 2 | `id` | hotspot id |
| 22 | 2 | `dest` | destination room id (door) or door id (bolt) |
| 24 | 2 | `nbrPts` | polygon point count |
| 26 | 2 | `ptsOfst` | index of the point array (4 bytes per point) |
| 28 | 2 | `type` | 0 normal, 1 door, 2 shuttable door, 3 lockable door, 4 bolt, 5 nav area |
| 30 | 2 | `groupID` | documented unused |
| 32 | 2 | `nbrScripts` | scripts attached |
| 34 | 2 | `scriptRecOfst` | documented unused |
| 36 | 2 | `state` | selected state |
| 38 | 2 | `nbrStates` | state count |
| 40 | 2 | `stateRecOfst` | index of the state array (8 bytes per state) |
| 42 | 2 | `nameOfst` | offset of the name `PString` |
| 44 | 2 | `scriptTextOfst` | offset of the script `CString` |
| 46 | 2 | `alignReserved` | filler; `0` |

A point is `sint16 y`, `sint16 x` (4 bytes). `nbrPts` and `nbrStates` of 0 mean
the corresponding array is absent.

### Hotspot state / `StateRec` (8 bytes each)

Array at `stateRecOfst`, `nbrStates` entries, stride 8.

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 2 | `pictID` | picture shown when this state is active |
| 2 | 2 | `reserved` | filler; `0` |
| 4 | 2 | `picLoc.y` | **offset from** the hotspot's `loc.y` |
| 6 | 2 | `picLoc.x` | **offset from** the hotspot's `loc.x` |

### Loose prop / `LPropRec` (24 bytes each, linked)

Reached from `firstLProp` and followed through `link.nextOfst`; at most
`nbrLProps` records are visited.

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | `link.nextOfst` (next record, `0` = end) |
| 2 | 2 | `link.reserved` |
| 4 | 4 | `propSpec.id` (32-bit unsigned in practice) |
| 8 | 4 | `propSpec.crc` |
| 12 | 4 | `flags` |
| 16 | 4 | `refCon` |
| 20 | 2 | `loc.y` |
| 22 | 2 | `loc.x` |

The 1999 reference declares `propSpec.id` as `sint32`, but prop ids routinely
exceed `i32::MAX` (the corpus contains `0xA26F9DE3`) and OpenPalace reads it
unsigned, so it is exposed as `u32`.

**Traversal, not stride.** In the live corpus loose props are written 48 bytes
apart even though a record is 24 bytes (the 24 bytes between records are zero),
and the server threads `nextOfst` so that `firstLProp` points at the *last*
array entry and each link steps *backwards*. None of that matters to the walker
because it follows the link. pserver does the opposite: it packs records at
stride 24 and writes `nextOfst = 0` for every one. The walker detects a `0` link
before the declared count is reached and falls back to the packed stride,
recording a `LinkedFallback` warning.

### Draw command / `DrawRecord` (10-byte header + operand, linked)

Reached from `firstDrawCmd` and followed through `link.nextOfst`; at most
`nbrDrawCmds` records are visited.

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | `link.nextOfst` (next record, `0` = end) |
| 2 | 2 | `link.reserved` |
| 4 | 2 | `command \| (flags << 8)` |
| 6 | 2 | `cmdLength` |
| 8 | 2 | `dataOfst` (vestigial) |
| 10 | `cmdLength` | `cmdData` — the operand |

`command` is a `DC_*` opcode (0 path, 1 shape, 2 text, 3 detonate, 4 delete,
5 ellipse); `flags` carries `DF_LAYER_FRONT` (0x80), `DF_USE_FILL` (0x01) and
`DF_IS_ELLIPSE` (0x40). The 1999 reference, pserver's `Draw::Serialise` ("data
offset is always 10") and QPalace's streaming reader all agree that the operand
follows the header; `dataOfst` is only "used publicly" and is therefore ignored
when locating the operand (kept raw).

For path/shape/ellipse operands the geometry is decoded (OpenPalace
`PalaceDrawRecord.readData`, QPalace `operator>>(QDataStream&, QPDraw*)`):

```text
sint16 penSize
sint16 numPoints            // segments; (numPoints + 1) points follow
uint8  r, r, g, g, b, b     // pen colour, each channel duplicated
Point  points[numPoints + 1]
uint8  lineAlpha, r, g, b   // optional PC5 tail
uint8  fillAlpha, r, g, b   // optional PC5 tail
```

### Record sizes (confirmed)

| Structure | Bytes |
|---|---|
| `RoomRec` header | 40 |
| `PictureRec` / image overlay | 12 |
| `Hotspot` | 48 |
| `StateRec` / hotspot state | 8 |
| point (`sint16 y`, `sint16 x`) | 4 |
| `LPropRec` loose prop | 24 (traversal via link) |
| `DrawRecord` header | 10 (+ `cmdLength` operand) |

### Traversal summary

| Data | Reached from | Count | Method |
|---|---|---|---|
| room name / background / artist / password | `*NameOfst`, `passwordOfst` | — | `PString` at offset |
| image overlays | `pictureOfst` | `nbrPictures` | fixed array, stride 12 |
| hotspots | `hotspotOfst` | `nbrHotspots` | fixed array, stride 48 |
| hotspot points | `ptsOfst` | `nbrPts` | fixed array, stride 4 |
| hotspot states | `stateRecOfst` | `nbrStates` | fixed array, stride 8 |
| hotspot name / script | `nameOfst` / `scriptTextOfst` | — | `PString` / `CString` at offset |
| loose props | `firstLProp` | `nbrLProps` | linked list via `nextOfst` |
| draw commands | `firstDrawCmd` | `nbrDrawCmds` | linked list via `nextOfst` |

### Tolerant parsing and the corpus

Every derived offset is bounds-checked before use; a bad offset degrades the
affected sub-structure and appends a `RoomWarning` — it never panics and never
invents data. Only a truncated header or `varBuf` is a hard `WireError`. The raw
buffer is always kept as `RoomDesc::var_data`.

Validated against `$CORPUS/payloads_all/` (799 real payloads harvested from a
live PC5 server):

```text
799 files, 804 records, 0 errors, 0 records with warnings
4 files concatenate 2–3 room records back to back:
  11021.bin (2), 11054.bin (2), 11060.bin (3), 4679.bin (2)
  → decode_stream() splits them by parsing one record and resyncing on the next
    plausible RoomRec within a small window past its end.
```

The replay test (`crates/palace-room/tests/corpus_replay.rs`) runs this and
asserts zero failures and zero warnings; it is skipped when the corpus is
absent, and `PALACE_ROOM_CORPUS` points it elsewhere.

### Fields that remain uncertain

These are recorded as raw bytes rather than guessed at:

* **Hotspot polygon points.** The 1999 reference and OpenPalace treat the
  points as absolute screen coordinates, but 3295 of the corpus's 3484 polygons
  contain negative or > 2048 values that only resolve when added to the
  hotspot's `loc`. `StateRec.picLoc` is explicitly an offset from `loc`; the
  polygon points appear to be too. The parser exposes the raw `i16`s and leaves
  the interpretation to the consumer.
* **`scriptEventMask`** is `0` for every hotspot in the corpus; the reference
  client derives event handlers by parsing the script text instead.
* **`scriptRecOfst` / `nbrScripts`** point at an array whose record layout is
  documented as unknown; only the offset is retained.
* **`secureInfo`, `refCon`, `groupID`** are documented as unused; values are
  retained raw.
* **`DrawRecord.dataOfst`** is vestigial (see above).
* **24 unexplained zero bytes** follow each live loose-prop record; traversal by
  link makes them irrelevant, so they are left as-is in `var_data`.

---

## Faces and avatars

Every user record carries two small fields that together decide what the user
looks like when they are not wearing any props:

| Field | Meaning |
|---|---|
| `face` | which face, 0..=12 |
| `color` | which colour variant, 0..=15 |

They index a built-in sprite sheet that every Palace client ships — the green
smileys in the "Choose an Avatar" dialog. The layout is a grid where **`face` is
the column and `color` is the row**. The reference draws it directly:

```actionscript
<s:Group horizontalScrollPosition="{user.face * 45}" verticalScrollPosition="{user.color * 45}">
  <s:BitmapImage source="@Embed(source='assets/faces/defaultsmileys.png')" />
```

**Cell geometry.** In the reference `defaultsmileys.png` the cells are 45px, but
the face art inside each one is 42×42 at offset (1,1), with a transparent border
around it. The reference compensates by drawing the 45px cell as a 44×44 bitmap at
`(-1,-1)`, so the art lands at the avatar's origin.

This client ships a **repacked** sheet, `crates/palace-render/assets/smileys.png`:
13 columns × 16 rows of 42×42 with the border trimmed, so a cell address is simply
`(color * 42, face * 42)` and it is drawn at offset `(0, 0)` with no fudge.
`tools/extract-smileys.py` regenerates it from the reference sheet.

**When the built-in face is hidden.** If a user wears a prop whose header carries
the `head` flag, the face is not drawn — the worn head replaces it. The reference
makes this explicit:

```actionscript
private function checkFaceProps():void {
    var showFace:Boolean = true;
    for each prop: if (prop.head) showFace = false;
    this.showFace = showFace;
}
```

Our equivalent is `PropHeader::is_head` in `palace-prop`. A user's other props
(hands, bodies, accessories) do not suppress the face.

**These values are not trustworthy.** They come from the server, so a client must
clamp them rather than index a sheet with whatever arrived.

## Opcodes discovered

75 opcodes are named, taken from the 1999 protocol reference, Taj's
`MessageTypes.cs` and QPalace's `message.hpp`. The `observed live` column marks
the ones this server actually sent during a probe session (12).

| mnemonic | name | value | observed live |
|---|---|---|---|
| `rep2` | ALTLOGONREPLY | `0x72657032` |  |
| `aAst` | ASSETNEW | `0x61417374` |  |
| `qAst` | ASSETQUERY | `0x71417374` |  |
| `rAst` | ASSETREGI | `0x72417374` |  |
| `sAst` | ASSETSEND | `0x73417374` |  |
| `auth` | AUTHENTICATE | `0x61757468` |  |
| `autr` | AUTHRESPONSE | `0x61757472` |  |
| `blow` | BLOWTHRU | `0x626c6f77` |  |
| `durl` | DISPLAYURL | `0x6475726c` |  |
| `ryit` | DIYIT | `0x72796974` |  |
| `lock` | DOORLOCK | `0x6c6f636b` |  |
| `unlo` | DOORUNLOCK | `0x756e6c6f` |  |
| `draw` | DRAW | `0x64726177` |  |
| `sInf` | EXTENDEDINFO | `0x73496e66` |  |
| `fnfe` | FILENOTFND | `0x666e6665` |  |
| `qFil` | FILEQUERY | `0x7146696c` |  |
| `sFil` | FILESEND | `0x7346696c` |  |
| `gmsg` | GMSG | `0x676d7367` |  |
| `HTTP` | HTTPSERVER | `0x48545450` | yes |
| `cLog` | INITCONNECTION | `0x634c6f67` |  |
| `kill` | KILLUSER | `0x6b696c6c` |  |
| `rLst` | LISTOFALLROOMS | `0x724c7374` | yes |
| `uLst` | LISTOFALLUSERS | `0x754c7374` | yes |
| `bye` | LOGOFF | `0x62796520` |  |
| `regi` | LOGON | `0x72656769` | yes |
| `sErr` | NAVERROR | `0x73457272` |  |
| `NOOP` | NOOP | `0x4e4f4f50` |  |
| `FPSq` | PICTDEL | `0x46505371` |  |
| `pLoc` | PICTMOVE | `0x704c6f63` |  |
| `nPct` | PICTNEW | `0x6e506374` |  |
| `sPct` | PICTSETDESC | `0x73506374` |  |
| `ping` | PING | `0x70696e67` |  |
| `pong` | PONG | `0x706f6e67` |  |
| `dPrp` | PROPDEL | `0x64507270` |  |
| `mPrp` | PROPMOVE | `0x6d507270` |  |
| `nPrp` | PROPNEW | `0x6e507270` |  |
| `sPrp` | PROPSETDESC | `0x73507270` |  |
| `resp` | RESPORT | `0x72657370` |  |
| `rmsg` | RMSG | `0x726d7367` |  |
| `room` | ROOMDESC | `0x726f6f6d` | yes |
| `endr` | ROOMDESCEND | `0x656e6472` | yes |
| `navR` | ROOMGOTO | `0x6e617652` |  |
| `nRom` | ROOMNEW | `0x6e526f6d` |  |
| `sRom` | ROOMSETDESC | `0x73526f6d` |  |
| `down` | SERVERDOWN | `0x646f776e` |  |
| `sinf` | SERVERINFO | `0x73696e66` | yes |
| `init` | SERVERUP | `0x696e6974` |  |
| `smsg` | SMSG | `0x736d7367` |  |
| `opSd` | SPOTDEL | `0x6f705364` |  |
| `coLs` | SPOTMOVE | `0x636f4c73` |  |
| `opSn` | SPOTNEW | `0x6f70536e` |  |
| `opSs` | SPOTSETDESC | `0x6f705373` |  |
| `sSta` | SPOTSTATE | `0x73537461` |  |
| `susr` | SUPERUSER | `0x73757372` |  |
| `talk` | TALK | `0x74616c6b` |  |
| `timy` | TIMYID | `0x74696d79` |  |
| `tiyr` | TIYID | `0x74697972` | yes |
| `pser` | TROPSER | `0x70736572` |  |
| `usrC` | USERCOLOR | `0x75737243` |  |
| `usrD` | USERDESC | `0x75737244` |  |
| `wprs` | USERENTER | `0x77707273` |  |
| `eprs` | USEREXIT | `0x65707273` |  |
| `usrF` | USERFACE | `0x75737246` |  |
| `rprs` | USERLIST | `0x72707273` | yes |
| `log` | USERLOG | `0x6c6f6720` |  |
| `uLoc` | USERMOVE | `0x754c6f63` |  |
| `usrN` | USERNAME | `0x7573724e` |  |
| `nprs` | USERNEW | `0x6e707273` | yes |
| `usrP` | USERPROP | `0x75737250` |  |
| `uSta` | USERSTATUS | `0x75537461` | yes |
| `vers` | VERSION | `0x76657273` | yes |
| `whis` | WHISPER | `0x77686973` |  |
| `wmsg` | WMSG | `0x776d7367` |  |
| `xtlk` | XTALK | `0x78746c6b` |  |
| `xwis` | XWHISPER | `0x78776973` |  |

Run `cargo run -p palace-probe -- --list-opcodes` for the same table.
