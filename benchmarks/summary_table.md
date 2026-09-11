| Pattern | Category | tgrep cold (ms) | tgrep warm (ms) | rg (ms) | Cold speedup | Warm speedup | Files |
|---------|----------|-----------------|-----------------|---------|-------------|-------------|-------|
| AtomicWriteFsync | rare | 200 | 60 | 2560 | 12.8x | 42.7x | 1 |
| CIStruct | caseinsensitive | 3170 | 80 | 2850 | 0.9x | 35.6x | 4698 |
| CITerminal | caseinsensitive | 2630 | 80 | 2350 | 0.9x | 29.4x | 2011 |
| DsmilHashIndex | rare | 170 | 50 | 1670 | 9.8x | 33.4x | 1 |
| FnMain | broad | 240 | 70 | 1710 | 7.1x | 24.4x | 1 |
| KeystoneTrigram | rare | 220 | 130 | 1610 | 7.3x | 12.4x | 1 |
| NonexistentXyz123 | rare | 190 | 60 | 1680 | 8.8x | 28.0x | 1 |
| QihseOptimization | rare | 270 | 50 | 2710 | 10.0x | 54.2x | 1 |
| RareNeedle | rare | 310 | 80 | 2150 | 6.9x | 26.9x | 1 |
| Return | broad | 570 | 80 | 3060 | 5.4x | 38.2x | 1357 |
| Struct | broad | 870 | 50 | 2080 | 2.4x | 41.6x | 238 |
| TgrepSegmentWriter | rare | 260 | 70 | 1420 | 5.5x | 20.3x | 1 |
| UseStd | broad | 310 | 70 | 2590 | 8.4x | 37.0x | 1 |
| WalCheckpointReplay | rare | 200 | 60 | 2170 | 10.8x | 36.2x | 1 |
| WordMain | word | 380 | 60 | 2780 | 7.3x | 46.3x | 243 |
| WordNonexistent | word | 1690 | 60 | 2240 | 1.3x | 37.3x | 1 |
| WordReturn | word | 260 | 90 | 2790 | 10.7x | 31.0x | 420 |
| WordStruct | word | 430 | 100 | 2070 | 4.8x | 20.7x | 77 |
| WordTerminal | word | 520 | 70 | 2330 | 4.5x | 33.3x | 679 |
