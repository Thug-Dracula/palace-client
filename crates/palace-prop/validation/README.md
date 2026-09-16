# Verbatim validation output

Raw output of the corpus run and the differential, captured with the committed
code. Re-run the commands in the crate README §9 and diff against these; the only
things expected to change are byte counts if the corpus grows.

## `prop-tool inventory` — every local source

```
$HOME/palace-corpus/pserver.prp	8-bit=180138	16-bit=0	20-bit=127	s20-bit=387	32-bit=2	failed=7	endian={"little": 180654}	pixels=349746144	sources=180661
	FAIL $HOME/palace-corpus/pserver.prp#969004551 [8-bit]: 8-bit RLE row 39 ran past the end of the scanline
	FAIL $HOME/palace-corpus/pserver.prp#1011591689 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1011591794 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1048832035 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1048832056 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1051394783 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1062038429 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
$HOME/palace-corpus/props_harvested	8-bit=41478	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 41478}	pixels=80301408	sources=41478
$HOME/palace-corpus/props_all	8-bit=2506	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 2506}	pixels=4851616	sources=2506
$HOME/palace-corpus/props_from_live	8-bit=1804	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 1804}	pixels=3492544	sources=1804
$HOME/palace-corpus/props_recovered2	8-bit=43	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 43}	pixels=83248	sources=43
$HOME/palace-corpus/props_recovered3	8-bit=253	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 253}	pixels=489808	sources=253
$HOME/palace-corpus/props_recovered4	8-bit=273	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 273}	pixels=528528	sources=273
$HOME/palace-corpus/props_recovered6	8-bit=18	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 18}	pixels=34848	sources=18
$HOME/palace-corpus/props_from_capture	8-bit=332	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 332}	pixels=642752	sources=332
$HOME/palace-corpus/props_harvest_final	8-bit=504	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 504}	pixels=975744	sources=504
$HOME/palace-corpus/props_test	8-bit=2	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 2}	pixels=3872	sources=2
TOTAL	8-bit=227351	16-bit=0	20-bit=127	s20-bit=387	32-bit=2	failed=7	endian={"little": 227867}	pixels=441150512
	FAIL $HOME/palace-corpus/pserver.prp#969004551 [8-bit]: 8-bit RLE row 39 ran past the end of the scanline
	FAIL $HOME/palace-corpus/pserver.prp#1011591689 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1011591794 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1048832035 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1048832056 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1051394783 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
	FAIL $HOME/palace-corpus/pserver.prp#1062038429 [8-bit]: 8-bit RLE tripped the reference runaway guard at row 43
```

## Tiered detail: the six other rosters in the corpus

```
$HOME/palace-corpus/reference/prp-variants/pserver_full.prp	8-bit=42325	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=136513	endian={"little": 42325}	pixels=81941200	sources=178838
$HOME/palace-corpus/reference/prp-variants/pserver_full2.prp	8-bit=178316	16-bit=0	20-bit=127	s20-bit=387	32-bit=2	failed=7	endian={"little": 178832}	pixels=346218752	sources=178839
$HOME/palace-corpus/reference/prp-variants/pserver_src_frozen.prp	8-bit=178334	16-bit=0	20-bit=127	s20-bit=387	32-bit=2	failed=7	endian={"little": 178850}	pixels=346253600	sources=178857
$HOME/palace-corpus/reference/prp-variants/prp_now.prp	8-bit=42052	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 42052}	pixels=81412672	sources=42052
$HOME/palace-corpus/reference/prp-variants/pserver_new.prp	8-bit=42325	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 42325}	pixels=81941200	sources=42325
$HOME/palace-corpus/BACKUPS_20260912_102742/server_pserver.prp	8-bit=435	16-bit=0	20-bit=0	s20-bit=0	32-bit=0	failed=0	endian={"little": 435}	pixels=842160	sources=435
```

(`pserver_full.prp` is the corrupt rebuild described in the crate README §6: 136,513
of its 178,839 records are shifted by 16 bytes and are rejected cleanly.)

## `diff_corpus.py` — 227,874 props, three Python oracles

```
# differential over 227874 props (digests)

## Rust vs OpenPalace (ActionScript) oracle
  20-bit       compared=127, match=127, prop_decoder_skipped_format=127
  32-bit       compared=2, match=2, prop_decoder_skipped_format=2
  8-bit        both_reject=7, compared=227351, match=227351, oracle_error=7, prop_decoder_compared=227346, prop_decoder_error=5, prop_decoder_match=227346
  s20-bit      compared=387, match=387, prop_decoder_skipped_format=387

## Rust vs prop_decoder.py (8-bit third-party oracle)
  20-bit       skipped=127 (prop_decoder.py supports 8-bit only)
  32-bit       skipped=2 (prop_decoder.py supports 8-bit only)
  8-bit        compared=227346 match=227346 mismatch=0
  s20-bit      skipped=387 (prop_decoder.py supports 8-bit only)

## Taj-style port (informational: known divergences)
  20-bit       differing_channels=482726, differs=126, same_as_openpalace=1
  32-bit       same_as_openpalace=2
  8-bit        same_as_openpalace=227346
  s20-bit      differing_channels=2300233, differs=381, same_as_openpalace=6

TOTAL disagreements: 0

## 7 props both sides refuse (recorded, not counted as disagreements)
  /tmp/diffwork/prp_all/8-bit_39c1d607_19268.bin: oracle=8-bit RLE row overflow / rust=8-bit RLE row 39 ran past the end of the scanline
  /tmp/diffwork/prp_all/8-bit_3c4baa09_79113.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43
  /tmp/diffwork/prp_all/8-bit_3c4baa72_79123.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43
  /tmp/diffwork/prp_all/8-bit_3e83e823_88272.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43
  /tmp/diffwork/prp_all/8-bit_3e83e838_88273.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43
  /tmp/diffwork/prp_all/8-bit_3eab02df_89041.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43
  /tmp/diffwork/prp_all/8-bit_3f4d6b9d_96566.bin: oracle=8-bit RLE runaway (reference guard) / rust=8-bit RLE tripped the reference runaway guard at row 43

## prop_decoder.py raised on 5 prop(s) that both the Rust decoder and the reference oracle decoded (227346 props it did decode matched byte for byte)
   The Python decoder indexes the payload without a bounds check, so it
   cannot express the reference's end-of-payload tolerance (see the
   crate README). These are a documented oracle limitation, not a
   Rust failure, so they do not fail the run.
  /tmp/diffwork/prp_all/8-bit_3c4baa02_79112.bin: IndexError: index out of range
  /tmp/diffwork/prp_all/8-bit_3e893177_88279.bin: IndexError: index out of range
  /tmp/diffwork/prp_all/8-bit_3eaaeffc_89034.bin: IndexError: index out of range
  /tmp/diffwork/prp_all/8-bit_3f4d6bbf_96570.bin: IndexError: index out of range
  /tmp/diffwork/prp_all/8-bit_3f4d6bca_96572.bin: IndexError: index out of range
```

## `diff_prerendered.py` — pre-existing renders from another pipeline

```
pre-rendered comparison: matched=14 mismatched=0 no-source-blob=1 decode-failed=0
```
