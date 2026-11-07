| Pattern | Category | tgrep cold (ms) | tgrep warm (ms) | rg cold (ms) | rg warm (ms) | Cold speedup | Warm speedup | Files |
|---------|----------|-----------------|-----------------|--------------|--------------|-------------|-------------|-------|
| AtomicWriteFsync | rare | 200 | 60 | 2560 | 1870 | 12.8x | 31.2x | 1 |
| CIStruct | caseinsensitive | 3170 | 80 | 2850 | 3010 | 0.9x | 37.6x | 4698 |
| CITerminal | caseinsensitive | 2630 | 80 | 2350 | 2400 | 0.9x | 30.0x | 2011 |
| DsmilHashIndex | rare | 170 | 50 | 1670 | 1680 | 9.8x | 33.6x | 1 |
| FnMain | broad | 240 | 70 | 1710 | 1980 | 7.1x | 28.3x | 1 |
| KeystoneTrigram | rare | 220 | 130 | 1610 | 2010 | 7.3x | 15.5x | 1 |
| NonexistentXyz123 | rare | 190 | 60 | 1680 | 2030 | 8.8x | 33.8x | 1 |
| QihseOptimization | rare | 270 | 50 | 2710 | 1730 | 10.0x | 34.6x | 1 |
| RareNeedle | rare | 310 | 80 | 2150 | 2200 | 6.9x | 27.5x | 1 |
| Return | broad | 570 | 80 | 3060 | 2310 | 5.4x | 28.9x | 1357 |
| Struct | broad | 870 | 50 | 2080 | 1850 | 2.4x | 37.0x | 238 |
| TgrepSegmentWriter | rare | 260 | 70 | 1420 | 1880 | 5.5x | 26.9x | 1 |
| UseStd | broad | 310 | 70 | 2590 | 2120 | 8.4x | 30.3x | 1 |
| WalCheckpointReplay | rare | 200 | 60 | 2170 | 2180 | 10.8x | 36.3x | 1 |
| WordMain | word | 380 | 60 | 2780 | 2220 | 7.3x | 37.0x | 243 |
| WordNonexistent | word | 1690 | 60 | 2240 | 2090 | 1.3x | 34.8x | 1 |
| WordReturn | word | 260 | 90 | 2790 | 2400 | 10.7x | 26.7x | 420 |
| WordStruct | word | 430 | 100 | 2070 | 2540 | 4.8x | 25.4x | 77 |
| WordTerminal | word | 520 | 70 | 2330 | 2200 | 4.5x | 31.4x | 679 |

Ratios are rg/tgrep medians within each CSV condition. These are historical timings;
cold/warm label the first application-cache miss/repeated queries, not controlled filesystem caches.
