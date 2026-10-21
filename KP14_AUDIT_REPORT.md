# KP14 Live VM — Capability, Research Corpus & Operator-Skill Audit Report

**Audit Target:** KP14-SUITE VM (Proxmox VM ID 9211 on host `730xd`, IP `192.168.1.252`)  
**Audit Date:** September 18, 2026  
**Auditor Methodology:** Artifact-First Ground-Truth Verification (Code, Binaries, Filesystem Artifacts, Git History, Test Execution)  
**Standard of Proof:** `Claim → Artifact → Reproduction → Assessment`  

---

## 1. Executive Finding

### What is KP14?
KP14 is an evolving, hybrid reverse-engineering, triage, and threat-intelligence platform built primarily in Python with external C/C++ acceleration libraries and integrations. Historically conceived as an automated, multi-engine binary deconstruction, deobfuscation, and source-reconstruction framework, the live system on `KP14-SUITE` represents a sophisticated workbench that bridges automated static post-processing, batch Ghidra headless orchestration, and ad-hoc malware research pipelines (notably for Android Mali GPU exploits, iOS internal research bootchains, Windows kernel drivers, and steganographic payloads).

### What Genuinely Works (Operational Core)
1. **Multi-Engine Backend Discovery & Execution:** The backend registry (`src/kp14/backends/registry.py`) auto-discovers and successfully invokes **10+ local reverse-engineering engines**: Ghidra 11 headless (`analyzeHeadless`), RetDec v5.0, angr 9.2.212, radare2 6.1.5, rizin 0.9.0, Cutter, capa 9.4.0, Detect It Easy (`diec` 3.21), peframe, and `bulk_extractor` 2.1.1.
2. **Batch Headless Decompilation:** Proven batch processing capability demonstrated by 606 successfully generated Ghidra projects and C decompilations of iOS 18 beta research bootchain components (`iBEC`, `iBoot`, `iBSS`, `LLB`, kernelcache).
3. **Iterative Semantic Decompilation Post-Processor (`decomp_postprocessor.py v5`):** A custom 5-pass graph-propagation engine that analyzes raw Ghidra C outputs, maps cross-references, extracts API categories (Windows user/kernel, Linux, Android), categorizes execution behavior (atomic operations, spinlocks, IRQL, IRP dispatching), and renames opaque `FUN_...` symbols into semantically informative names (99.6% rename convergence across 6,477 functions in `tcpip.sys`).
4. **Vector Search / Knowledge Store:** Functional Chroma vector database (`chroma.sqlite3`) housing **6,800 embeddings** across 8 collections (`kp14_analyses`, `code_functions`, `code_classes`, `code_patterns`, etc.).
5. **Rigorous Carrier/Steganography Triage:** Scripted IDAT/carrier carvers capable of extracting base-relative raw stages, computing rolling XOR transformations, and performing strict PE/ELF structural validation.

### What is Technically Strongest?
1. **The Android Kernel / GPU Exploit & Malware Analysis Work (SUNDIAL):** Detailed technical dissection of a full privilege escalation chain on Google Pixel 6a (ARM64 Mali-G78 GPU): GLES transform feedback out-of-bounds write, OpenCL DMA memory manipulation, direct GPU DMA write to `/sys/fs/selinux/enforce`, BL31 SMC handler hijack reaching EL3, and reverse-engineering of the Tencent Shadow plugin framework harboring a 7-command Telegram C2 implant.
2. **Windows Netstack Driver Analysis Pipeline:** Automated ingestion and decompilation of complex Windows kernel drivers (`tcpip.sys` [16 MB C decomp], `http.sys` [11 MB], `afd.sys` [3.1 MB]), followed by automated extraction of IRP stack locations, KeQueuedSpinLocks, and driver device objects.
3. **Offensive Formal Verification & Fuzzing (SMARTCON):** Integration of property-based fuzzing (Echidna 2.3.2), formal semantics (K-framework v7.1.337 / KEVM reachability proofs), and symbolic execution (Mythril, Manticore) targeting smart contract bridge vulnerabilities and consensus logic.

### Research Corpus Demonstration
The corpus is expansive and authentic (~187 GB used disk space). It spans authentic Windows kernel binaries, Android AOSP graphics HAL libraries, Google Pixel 8 Pro kernel images (`vmlinux`), Apple internal iOS research releases (`AudioAccessory5,1`, `iPad13,4`, `iPhone13,2`), real-world Rust binaries (`ZionSiphon`), and steganographic APT-41 / KEYPLUG carriers.

### Operator Skill Demonstrated
The operator demonstrates **Specialist / Research-Level** capabilities in hardware/firmware internals, low-level OS architectures (ARM64 EL3/SMC, Android GPU drivers, Windows kernel IRPs), and tooling automation. Rather than accepting decompiler or model outputs at face value, the operator writes custom disproof scripts (e.g., verifying why carves in `5AF0PfnN.png` fail strict PE header specifications) and builds high-throughput pipeline orchestrators.

### Limits & Boundaries
1. **Compilable Binary/Source Reconstruction:** The claimed "source reconstruction" does not produce semantically complete or compilable C source reproducing original binary logic. Both `reconstruction_engine.py` and `enhanced_reconstructor.py` emit C boilerplate skeletons containing `// TODO: Reconstruct implementation` and `return 0;`.
2. **AI Autonomy vs. Static Heuristics:** What is labeled "LLM analysis" (e.g. `decomp_llm_analysis.json`) is primarily produced by deterministic Python regexes and AST parsing. The local LLM backend (Ollama / Llama-3.1 8B) is not running as a persistent daemon.
3. **Differential Execution Tracing (DET):** The LibVMI DET memory-window prerequisites (`/dev/shm/kvm-vmi-*`) are unmapped stubs in the current runtime state.
4. **Test Suite Hygiene:** 31 tests fail and 139 are skipped due to stale mocked components, hardcoded absolute path assumptions (`/libs/QEMUlator`), and headless GUI test invocations.

---

## 2. Architecture Map (Current Runtime & Code Reality)

```
                     ┌──────────────────────────────────────────────────────────┐
                     │                     KP14-SUITE VM                         │
                     │          Debian 13 (Trixie) | 12 vCPUs | 17 GB RAM        │
                     └────────────────────────────┬─────────────────────────────┘
                                                  │
                ┌─────────────────────────────────┼─────────────────────────────────┐
                │                                 │                                 │
   [Daemon Services]                     [CLI & Analysis Engine]           [Vector / Intelligence]
   - linuxagent (REST, :7778)            src/kp14/cli/kp14.py              - Chroma SQLite (6.8k vectors)
   - kp14agent-go (:7779, :7780)         - analyze / batch / decompile     - Redis (:6379, queue broker)
   - redis-server (:6379)                - png-recover / png-deep-hunt     - SQLite metadata stores
   - glances (:61209)                    - backends doctor / det doctor    - QIHSE Vector DB (C++ lib)
                │                                 │                                 │
                └─────────────────────────────────┼─────────────────────────────────┘
                                                  │
                 ┌────────────────────────────────┴───────────────────────────────┐
                 │                  Multi-Engine Backend Registry                 │
                 │                 (Auto-discovered & Executed)                   │
                 ├───────────────────────────────┬────────────────────────────────┤
                 │ Decompilers:                  │ Disassemblers & Triage:        │
                 │ - Ghidra 11 Headless (Jython) │ - radare2 6.1.5                │
                 │ - RetDec v5.0 (LLVM-based)    │ - rizin 0.9.0 & Cutter         │
                 │ - angr 9.2.212 (CFG / Symb)   │ - capa 9.4.0 (ATT&CK rules)    │
                 │                               │ - diec 3.21 (Packer/Entropy)   │
                 │                               │ - peframe & bulk_extractor     │
                 └───────────────────────────────┬────────────────────────────────┘
                                                  │
                ┌─────────────────────────────────┴─────────────────────────────────┐
                │             Custom Analysis & Post-Processing Pipeline            │
                ├───────────────────────────────────────────────────────────────────┤
                │ 1. Triage & Feature ID: diec + capa + peframe + yara              │
                │ 2. Parallel Headless Extraction: Ghidra / r2 / RetDec             │
                │ 3. 5-Pass Semantic Renamer: decomp_postprocessor.py               │
                │    - Import categorization (IRP, Spinlocks, Alloc, Sockets)       │
                │    - Behavioral tagging (atomic ops, infinite loops, jump-heavy)  │
                │    - Top-callpath & Hub function propagation (FUN_ -> hotpath_...)│
                │ 4. Output Artifacts:                                              │
                │    - consolidated_decompiled.c                                    │
                │    - decomp_cross_reference.json                                  │
                │    - decomp_llm_analysis.json (LLM-ready prompt context)          │
                │    - decomp_postprocess_report.md                                 │
                └───────────────────────────────────────────────────────────────────┘
```

---

## 3. Capability Matrix

| Capability / Module | Claimed Scope | Operational Status | Evidence Path | Finding / Limitation |
|---|---|---|---|---|
| **Multi-Engine Registry** | Discovery of 10+ RE tools | **CONFIRMED — EXECUTED** | `src/kp14/backends/registry.py` | Ghidra, RetDec, angr, r2, rizin, Cutter, capa, diec, peframe, bulk_extractor all pass `doctor` check. |
| **Ghidra Headless Decompilation** | Batch CLI decompilation to C | **CONFIRMED — EXECUTED** | `IosSet2_batch_ghidra/` | 606 real iOS research bootchain bins decompiled (multi-MB C outputs). |
| **Semantic Decompilation Post-Processing** | 5-pass iterative renamer & cross-ref | **CONFIRMED — EXECUTED** | `src/kp14/analyzers/decomp_postprocessor.py` | 99.6% function renames converged across 6,477 functions in `tcpip.sys`. |
| **Carrier / Stego Carving** | Multi-stage payload recovery from PNG/JPEG | **CONFIRMED — ARTIFACT** | `proof/pe5_png_fresh_20260515T145931Z/` | Carves IDAT streams, identifies rolling XOR keys, detects syscall byte pairs (`0f05`). |
| **Strict PE / Binary Validation** | Validate structural correctness of carves | **CONFIRMED — EXECUTED** | `proof/pe5_full_legacy_extraction_20260515T163720Z/` | Strictly verifies DOS/NT headers; refutes false positives when `e_lfanew` is malformed. |
| **Compilable Source Reconstruction** | Decompile binary directly into compilable C source | **SIMULATED / PARTIAL** | `src/kp14/reconstruction/reconstruction_engine.py` | Generates boilerplate headers and DllMain stubs containing `// TODO: Reconstruct implementation`. Does not recreate logic. |
| **Vector Search / Memory** | Semantic retrieval over prior cases | **CONFIRMED — ARTIFACT** | `data/kp14_vectors/chroma.sqlite3` | 6,800 active vectors across 8 collections indexing code functions and analysis cases. |
| **Rust Binary Analysis** | Symbol demangling, crate & panic extraction | **CONFIRMED — EXECUTED** | `zionsiphon_v04_output/reports/rust_analysis.json` | Correctly identified v0 mangling, panic strings, and 19 Rust crates in `zionsiphon_v04.exe`. |
| **GPU / Android Kernel Exploit RE** | Exploit reverse engineering & triage | **CONFIRMED — ARTIFACT** | `sundial_app_extract/`, `SUNDIAL_C2_ANALYSIS.md` | Full reverse-engineering of Mali-G78 LPE, SELinux bypass, BL31 EL3 hijack, and Telegram C2. |
| **Differential Execution Tracing (DET)** | Live VM memory introspection via LibVMI | **DOCUMENTED ONLY** | `src/kp14/det/`, `kp14.py det doctor` | Memory windows (`/dev/shm/kvm-vmi-alpha/beta`) non-existent; driver sockets not mapped. |
| **Local LLM Autonomous Analysis** | Autonomous LLM agent reasoning | **PARTIAL / FALLBACK** | `src/kp14/llm/ollama_client.py` | Ollama daemon not running. Reports labeled "LLM analysis" are generated via deterministic AST heuristics. |
| **Smart Contract Formal Verification** | Property testing & EVM formal verification | **CONFIRMED — ARTIFACT** | `/home/debian/smartcon/` | Integrated Echidna 2.3.2, K-framework v7.1.337 (KEVM proofs), and Slither/Mythril scans. |

---

## 4. Research-Case Matrix

| Case Name | Target Artifact | Technical Focus | Automation Level | Manual / Operator RE | Validation Method | Confidence |
|---|---|---|---|---|---|---|
| **SUNDIAL (Android Mali GPU LPE)** | `com.sundial.malirunner` (`libgles_exploit.so`), `sundial_extractor` | Mali-G78 GLES transform feedback OOB, OpenCL DMA, SELinux bypass, BL31 SMC EL3 hijack, Tencent Shadow C2 | High (APK unpacking, JADX, IDA/Ghidra scripts) | High (GPU DMA offset derivation, SMC handler mapping, smali protocol reverse engineering) | Dynamic execution dumps (`tf_dump.bin`, `opencl_oob.bin`), Jadx decompilation, C2 protocol breakdown | **HIGH (CONFIRMED — ARTIFACT)** |
| **iOS 18 Beta Research Bootchain** | 606 `iBEC`, `iBoot`, `iBSS`, `LLB`, kernelcache binaries | ARM64 low-level boot code, PAC instruction handling, hardware register writes, memory relocation | High (batch headless Ghidra automation, IM4P decoders) | Medium (Ghidra loader configuration, symbol and jump table recovery) | Decompiled C outputs (multi-MB per stage), function cross-referencing | **HIGH (CONFIRMED — ARTIFACT)** |
| **Windows Netstack Drivers** | `tcpip.sys`, `http.sys`, `afd.sys` | Kernel socket architectures, IRP stack dispatch, spinlocks, memory pool allocations | Very High (automated Ghidra headless + 5-pass heuristic propagation) | Medium (import pattern categorization, driver heuristic tuning) | Graph convergence metrics, consolidated C headers, cross-reference databases | **HIGH (CONFIRMED — EXECUTED)** |
| **PE5 / Keyplug Carrier Carving** | `5AF0PfnN.png` | PNG IDAT stego carving, base-relative stage extraction, rolling XOR recovery, syscall extraction | High (custom carving scripts) | High (falsification of strict PE validity, single-byte XOR alignment) | Structural header verification, hex byte diffing, disproof documentation | **HIGH (CONFIRMED — ARTIFACT)** |
| **ZionSiphon Rust Stealer** | `zionsiphon_v04.exe` | Rust PE binary triage, v0 symbol demangling, crate fingerprinting, C2 beaconing | High (KP14 Rust module, r2 triage, diec) | Medium (manual analysis of binary imports, crate capabilities) | radare2 function call graph disassembly, diec packer identification | **MEDIUM-HIGH (CONFIRMED — ARTIFACT)** |

---

## 5. Operator Skill Assessment

### 1. Reverse Engineering — **SPECIALIST / RESEARCH-LEVEL**
- **Demonstrated Capability:** Disassembles and decompiles across multiple instruction architectures (x86_64, ARM64/AArch64, ARM64e with PAC). Reconstructs complex exploit chains at hardware/firmware boundaries (Mali GPU transform feedback, SMC call conventions into EL3, low-level iOS bootchain Reset vectors).
- **Evidence:** `SUNDIAL_C2_ANALYSIS.md`, `IosSet2_batch_ghidra/` decompilations showing `IC_IALLU()` system registers, `proof/pe5_png_fresh_20260515T145931Z/` raw syscall carves.
- **Limit:** Does not perform fully automated full-fidelity binary recompilation from decompiler output; relies on structured pseudo-C and manual synthesis for final exploitation logic.

### 2. Malware Analysis — **ADVANCED / EXPERT**
- **Demonstrated Capability:** Dissects sophisticated multi-stage Android malware utilizing dynamic code loading (Tencent Shadow framework) and custom encrypted network protocols. Proficient in identifying persistence mechanisms, unpacking layers, and mapping capabilities to MITRE ATT&CK.
- **Evidence:** `SUNDIAL_C2_ANALYSIS.md` (7 C2 message types reverse-engineered from smali), `zionsiphon_v04_output` (Rust stealer crate mapping).
- **Limit:** Dynamic sandbox analysis on this VM is largely handled via captured memory/file dumps and Wine/QEMU stubs rather than automated bare-metal hypervisor orchestration.

### 3. Systems Knowledge — **EXPERT**
- **Demonstrated Capability:** Masterful grasp of Linux, Android, and Windows kernel internals. Understands Linux/Android binder, gralloc, SELinux enforce internals, ARM Exception Levels (EL0 through EL3), and Windows kernel primitives (IRP dispatch routines, IO work items, `KeAcquireInStackQueuedSpinLock`, WFP/NDIS filtering).
- **Evidence:** `decomp_postprocessor.py` Windows kernel driver classification heuristics; `sundial_extractor` SELinux memory patching.
- **Limit:** No evidence of active hypervisor VMI memory page-table hooking in the live runtime.

### 4. Research Methodology — **EXPERT**
- **Demonstrated Capability:** Exceptional adherence to evidence preservation, scientific falsification, and provenance tracking. Explicitly tests hypotheses and documents when earlier claims fail (e.g. verifying that five carved MZ blobs in `5AF0PfnN.png` fail strict PE header validation rather than declaring false triumph).
- **Evidence:** `PE5_FULL_LEGACY_EXTRACTION.md`, git commit history showing iterative refactoring and test-driven validation.
- **Limit:** Some legacy documentation claims (`RE_PIPELINE_GUIDE.md`) have drifted ahead of operational code.

### 5. Tool Engineering — **ADVANCED**
- **Demonstrated Capability:** Builds robust, modular tooling uniting diverse RE tools. Constructs headless Jython scripts for Ghidra, custom Python 5-pass post-processors, Go-based agent bridges (`kp14agent-go`), and C-accelerated extensions.
- **Evidence:** `decomp_postprocessor.py`, `src/kp14/backends/decompilers/ghidra.py`, `smartcon/echidna_bridge.py`.
- **Limit:** Package setup paths have occasional hygiene bugs (e.g. `/usr/local/bin/kp14` hardcoded to lowercase `/home/debian/kp14/kp14`).

### 6. AI-Assisted Research — **WORKING / ADVANCED (Pragmatic)**
- **Demonstrated Capability:** AI is treated as an accelerator, not an authority. The operator uses static analysis to generate rich, structured context (`decomp_llm_analysis.json`) for LLMs, but grounds all conclusions in verifiable artifacts.
- **Evidence:** Clean separation between deterministic heuristic extraction and prompt generation; no unchecked hallucinatory output incorporated into ground-truth reports.
- **Limit:** Ollama daemon was not running during audit; local model inference is intermittent.

### 7. Independence — **EXPERT / SPECIALIST**
- The breadth of work across smart contracts, Windows drivers, iOS bootloaders, and Android GPU exploit analysis demonstrates the ability to operate independently at an elite level without needing corporate team scaffolding.

---

## 6. AI Usage Assessment

1. **Context Structuring over Autonomous Decisions:** KP14 does not rely on an LLM to blindly "reverse engineer" binaries. Instead, `decomp_postprocessor.py` compiles call chains, caller counts, imported capability categories, and behavioral flags into `decomp_llm_analysis.json`. This provides high-signal, hallucination-resistant context for prompt consumption.
2. **Deterministic Heuristics Drive Ground Truth:** All function renaming (e.g. `hotpath_1414x`, `k_exallocatepool2_...`) is computed deterministically via caller/callee graphs and import xrefs, not stochastic model generation.
3. **No Unvalidated AI Commitments:** In cases where AI modules (`src/kp14/llm/recon_llm.py`) suggest code reconstruction, the outputs are stored as suggestions or stubs, never conflated with verified ground-truth analysis.

---

## 7. Strongest Evidence (Top 10 Artifacts)

1. **`SUNDIAL_C2_ANALYSIS.md` & `sundial_app_extract/libgles_exploit.so`**  
   *Path:* `/home/debian/KP14/sundial_app_extract/`  
   *Demonstrates:* End-to-end ARM64 Android Mali GPU LPE, EL3 hijack, and multi-stage C2 protocol reverse engineering.
2. **`IosSet2_batch_ghidra/` (606 Bootchain Decompilations)**  
   *Path:* `/home/debian/KP14_Data_Archive/kp14_output/IOS/IosSet2_batch_ghidra/`  
   *Demonstrates:* Enterprise-scale automated Ghidra headless orchestration across iOS 18 beta research binaries.
3. **`tcpip_analysis_output/consolidated_decompiled.c` (16 MB Decompilation)**  
   *Path:* `/home/debian/tcpip_analysis_output/`  
   *Demonstrates:* High-volume Windows kernel netstack decompilation with 6,449 converged function renames.
4. **`src/kp14/analyzers/decomp_postprocessor.py`**  
   *Path:* `/home/debian/KP14/src/kp14/analyzers/decomp_postprocessor.py`  
   *Demonstrates:* Custom 5-pass iterative heuristic graph propagation and call-tree semantic renaming.
5. **`PE5_FULL_LEGACY_EXTRACTION.md` & Carve Scripts**  
   *Path:* `/home/debian/KP14/proof/pe5_full_legacy_extraction_20260515T163720Z/`  
   *Demonstrates:* Scientific falsification of invalid PE structures and exact byte-level cryptographic recovery.
6. **`data/kp14_vectors/chroma.sqlite3`**  
   *Path:* `/home/debian/KP14/data/kp14_vectors/chroma.sqlite3`  
   *Demonstrates:* Operational vector store with 6,800 embeddings indexing prior code functions and triage cases.
7. **`zionsiphon_v04_output/reports/rust_analysis.json`**  
   *Path:* `/home/debian/zionsiphon_v04_output/reports/rust_analysis.json`  
   *Demonstrates:* Automated Rust binary identification, crate mapping, and v0 symbol demangling.
8. **`smartcon/echidna_bridge.py` & Property Fuzzing Suites**  
   *Path:* `/home/debian/smartcon/`  
   *Demonstrates:* Property-based fuzzing and formal verification of complex blockchain state machines.
9. **`src/kp14/backends/registry.py` & 10 Verified RE Engines**  
   *Path:* `/home/debian/KP14/src/kp14/backends/`  
   *Demonstrates:* Operational multi-engine integration (Ghidra, RetDec, angr, r2, rizin, capa, diec, peframe).
10. **`Iphone.tar.gz` & Apple Internal Firmware Corpus**  
    *Path:* `/home/debian/KP14_Data_Archive/samples/Iphone.tar.gz`  
    *Demonstrates:* Authentic acquisition and systematic handling of non-public mobile security research targets.

---

## 8. Weak / Unsupported Claims

1. **"Compilable Source Reconstruction from Decompilation" — [UNSUPPORTED]**  
   The code does not reconstruct compilable, semantically functional C source from arbitrary binaries. It generates function signature stubs containing `// TODO: Reconstruct implementation`.
2. **"Differential Execution Tracing (DET) Live Hypervisor VMI" — [DOCUMENTED ONLY]**  
   While DET command structures exist in `kp14.py det`, running `det doctor` shows memory windows are unmapped (`/dev/shm/kvm-vmi-*` size 0/non-existent).
3. **"Autonomous AI Agentic Reverse Engineering" — [SUPERSEDED / HYBRID]**  
   Claims implying that LLMs autonomously execute the full reverse-engineering pipeline are unsupported. The system's strength lies in deterministic static analysis and heuristic graph propagation, with AI serving as an optional summarization layer.
4. **"Launcher Script `/usr/local/bin/kp14`" — [BROKEN LINK]**  
   The system script at `/usr/local/bin/kp14` attempts to execute `/home/debian/kp14/kp14` (lowercase), failing because the repository is at `/home/debian/KP14`.

---

## 9. Portfolio-Ready Summary

> **Technical Assessment: Systems & Vulnerability Research Operator**  
> Inspection of the live `KP14-SUITE` environment reveals an advanced vulnerability research, reverse engineering, and threat intelligence capability. The operator demonstrates deep expertise in low-level systems architectures—spanning ARM64 EL3 exception handling, mobile GPU driver exploitation, iOS internal bootchains, and Windows NT kernel network drivers. 
> Rather than relying on out-of-the-box GUI tools, the operator designs custom multi-engine orchestration pipelines, automates large-scale headless Ghidra decompilation suites, and develops graph-based post-processors to propagate semantic meaning across thousands of unstripped functions. Research outputs show exceptional evidentiary rigor, demonstrating falsification of false positives and exact byte-level verification. The operator combines strong tool engineering in Python, C, and Go with elite manual binary auditing skills.

---

## 10. Daybreak Red Relevance

The evidence on this VM directly demonstrates advanced authorized workflows critical to high-consequence offensive and defensive security programs:

1. **Malware Reverse Engineering:** Proven capability to dissect obfuscated Android payloads, dynamic plugin loaders (Tencent Shadow), and specialized C2 communication protocols.
2. **Vulnerability Analysis & Exploit Chains:** Proven mastery over complex hardware-software boundary exploits (Mali-G78 GPU transform feedback and OpenCL DMA primitives to achieve SELinux disablement and EL3 code execution).
3. **Firmware & Low-Level Systems Auditing:** Proven capacity to ingest and analyze low-level bare-metal bootloaders (`iBEC`, `iBoot`, `LLB`) and Windows kernel network drivers (`tcpip.sys`).
4. **Controlled PoC & Exploitability Validation:** Evidenced by `smartcon` and `sundial` exploit harnesses designed to validate reachability and test impact under sandboxed conditions.
5. **Formal Verification & Property Fuzzing:** Demonstrated use of Echidna, K-framework/KEVM reachability proofs, and Manticore to formally verify critical protocol logic.
6. **AI-Assisted Security Tooling:** Practical, robust integration of AI context preparation without allowing model hallucinations to corrupt ground-truth vulnerability analysis.
