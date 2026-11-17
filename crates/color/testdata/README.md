# `color` test data

| File | Captured or constructed | What |
|---|---|---|
| `m27q.edid` | **Captured** 2026-11-17 from the author's Gigabyte M27Q (Windows' registry, `Enum\DISPLAY\GBT270D\…\Device Parameters\EDID`) | the 128-byte base block, with its serial number blanked — bytes 12–15 zeroed and the serial-string descriptor at 108 replaced by `0` — the extension count set to 0 and the checksum redone. The colour bytes (23–34) are as the monitor sent them. |
