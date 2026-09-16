# Test fixtures

Real prop blobs, copied byte-for-byte out of the local corpus. They exist so the
crate can test against genuine 1990s data without the 700 MB corpus being present,
and so the pinned pixel values in `tests/fixtures.rs` are citable.

Nothing here was edited, re-encoded or "fixed". `malformed_rle_overflow.bin` and
`truncated_final_run.bin` in particular are left exactly as the corpus has them,
because their whole purpose is to pin what a broken prop does.

| File | Bytes | Format | Source record | SHA-256 (first 16) |
|---|---|---|---|---|
| `8bit_head_rare.bin` | 227 | 8-bit, flags `0x000a` (HEAD\|RARE) | `props_harvested/1001438438_unnamed.bin` | `9036c5cf5ee61232` |
| `8bit_avatar.bin` | 2036 | 8-bit, flags `0x0002` (HEAD) | `pserver.prp` record index 5200, id `0x00000008` | `cdeeead87d9ed555` |
| `s20_bit.bin` | 40 | S20, all-transparent | `pserver.prp`, smallest S20 prop | `01b70e6103e5ac58` |
| `s20_avatar.bin` | 2212 | S20, id `0x803d6c40` | `pserver.prp` | `a50c9f93bbda7458` |
| `20bit_bit.bin` | 1075 | 20-bit, id `0x00b79880` | `pserver.prp` | `8ac4aa12cee2c362` |
| `32bit_bit.bin` | 991 | 32-bit, id `0x8919cd0a` | `pserver.prp` | `13683f4cf344a6fa` |
| `malformed_rle_overflow.bin` | 1746 | 8-bit, rejected by both the reference and this crate | `pserver.prp` id `969004551` | `23bae5b992d2d269` |
| `truncated_final_run.bin` | 901 | 8-bit, payload ends one byte into the last row | `pserver.prp` id `1011591682` | `a73406430910511c` |

Full SHA-256 of every file:

```text
8ac4aa12cee2c36238cef5f2eda45a208aafb380d0922faee4ed0118c0e9652f  20bit_bit.bin
13683f4cf344a6fadab3431727a4517d4f39e037f4c13516e2682cabe2d34ce7  32bit_bit.bin
cdeeead87d9ed55556ea05c9dd8cf5f038b713253c0452d4f2965c84e6e89081  8bit_avatar.bin
9036c5cf5ee6123252dc940a796f2b24c37c5dac275acd5e31eb1a578830a299  8bit_head_rare.bin
23bae5b992d2d269ed2fc51d453cd21d68a2a570a825a6bae1b263e0d7b0aa76  malformed_rle_overflow.bin
a50c9f93bbda7458e1099c4e95eee992c85239d028d314d6117c6d8503ee9aab  s20_avatar.bin
01b70e6103e5ac58c1bf6d1a1f132d3efb7c70483fd2b2f2206ca998050e6265  s20_bit.bin
a73406430910511c4eb02e6e80d4b325fe0be2c94b995d81db907ae2613ebf39  truncated_final_run.bin
```

## The expected pixels

`tests/fixtures.rs` pins a handful of pixels per fixture. Those values come from
the ActionScript-reference oracle in `tools/oracle_prop.py`, which
`tools/diff_corpus.py` validates against the whole 227,874-prop corpus. They are a
regression lock on *this* implementation, not independent evidence about the
format — the corpus differential is the evidence.

## There is no 16-bit fixture

No genuine 16-bit prop exists anywhere in the local corpus, so there is nothing to
copy. The 16-bit decoder is exercised by synthetic fixtures built inline in
`src/codec/sixteen.rs` instead; see the crate README §7.
