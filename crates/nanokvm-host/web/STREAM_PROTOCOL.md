# NanoKVM HEVC HTTP stream

The `/api/kvm/stream` response uses `application/vnd.nanokvm.hevc` and remains
open for the lifetime of one browser capture session. All integers are unsigned
and use network byte order.

## Preamble

The response begins with one 24-byte preamble:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 8 | ASCII magic and version, `NKHEVC01` |
| 8 | 4 | Coded width |
| 12 | 4 | Coded height |
| 16 | 4 | Frames per second |
| 20 | 4 | Timestamp timescale, currently 90000 |

## Access units

Each HEVC Annex-B access unit is preceded by a 12-byte header:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | Payload length |
| 4 | 4 | Timestamp modulo 2^32 |
| 8 | 1 | Flags; bit 0 marks a keyframe |
| 9 | 3 | Reserved, zero |

The payload immediately follows the header. HTTP chunk boundaries have no
protocol meaning. Timestamps are contiguous for access units accepted by the
HTTP response; frames intentionally discarded by backpressure do not advance
the stream timeline. After a discard, the next emitted access unit is a
keyframe carrying VPS, SPS, and PPS.

Version 1 limits access units to 32 MiB. Browser implementations additionally
reject dimensions above 8192 by 4320, more than 4096 by 2160 total pixels, or
frame rates above 240 fps before allocating decoder resources.
