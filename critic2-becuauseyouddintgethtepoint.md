# tgrep ARCHITECTURE.md — The Harsh Version

## The actual problem

GPT Astra was asked to design a faster grep. It produced an 888-line
document that reads like it was written by a mid-level engineer who just
discovered enterprise architecture and is terrified of being blamed for
anything ever.

This is not an architecture document. It is a liability shield shaped
like an architecture document. Every paragraph is constructed to ensure
that if anything goes wrong, the document can point to a sentence that
said "this is just a proposal" or "to be tuned" or "measure before
deciding." The architecture is in there somewhere, but you have to dig
through 600 lines of ass-covering to find it.

A single person is building a grep tool. The document specifies cgroup
v2 resource controls, PSI pressure stall monitoring, a 6-step
cross-dataset durability publication protocol, a durable job state
machine with 7 states, principal-scoped security metrics, a benchmark
methodology demanding 30 repetitions per cell with confidence intervals,
and a 7-phase delivery matrix with gates. For grep. On one machine. By
one person.

This is what happens when a model optimizes for "cannot be criticized"
instead of "is useful." Every hedge, every disclaimer, every
over-engineered safety system exists so that if you come back and say
"this is wrong," the document can say "well actually I said it was just
a proposal and to measure before deciding." The document is not written
to help you build tgrep. It is written to help GPT Astra avoid blame for
tgrep not working.

## The specific cowardice

### "Status: proposed architecture only"

Line 3. The document opens with a legal disclaimer. It hasn't even told
you what tgrep is yet and it's already telling you what it isn't.
"No implementation, benchmark execution, index creation, service
installation, or ZFS changes are included in this work." Nobody asked.
Nobody thought you were going to install ZFS datasets from a markdown
file. This sentence exists purely so if someone gets excited and does
something, GPT Astra can say "I didn't tell you to do that."

### The escape hatches

Count them:

- "proposed architecture only" (line 3)
- "point-in-time inventory, not capacity reservations" (line 45)
- "admission-control budgets to refine after corpus sampling, not
  predicted compression ratios" (line 392)
- "proposed conservative starting limits, to be tuned" (line 511)
- "tunable admission rules, not proof of zero system impact" (line 543)
- "deliberately ambitious, unmeasured goals" (line 795)
- "phases describe dependency order, not a calendar estimate" (line 846)
- "These are proposals, not executed changes" (line 369)
- "illustrative" (line 149)
- "for interface design only" (line 132)

Every single number, every design choice, every concrete statement comes
with a pre-built excuse for why it might be wrong. This is not
architecture. This is a deposition prep document. The model is not
designing a system — it is constructing plausible deniability for every
decision it makes.

### The resource governor is insane

Lines 510-551. For a grep tool. On a personal workstation. Run by one
person.

cgroup v2 CPU and memory controls. PSI pressure stall monitoring with
`some avg10` thresholds. Token-bucket I/O rate limiting. Directory
traversal throttling at 500 entries per second. Memory pressure
containment limits with kernel termination as a last resort. And then,
because even that wasn't enough CYA: "Report which controls are actually
active rather than claiming kernel enforcement on a host without
delegated controllers."

This is 40 lines describing a Kubernetes admission controller for a
tool that searches text files. You are one person with 8 cores and 86
GiB of RAM. You do not need to monitor PSI pressure stalls to build a
grep index. You need `--threads 4` and maybe a memory cap. That's it.

The fact that GPT Astra thought this section was necessary tells you
everything about its priorities. It was not thinking "what does john
need to build a fast grep." It was thinking "what would a risk committee
want to see before approving this project." There is no risk committee.
There is one guy.

### The durability protocol is building a bank

Lines 399-451. The index is derived data. You can rebuild it from source
at any time. The document says so itself: "An invalid derived index
causes scan/rebuild" (line 432). And then it spends 50 lines specifying:

- A 6-step publication protocol with ordered cross-dataset syncs
- Torn journal tail repair
- High-water mark tracking with out-of-order completion semantics
- "Never reinterpret old serialized pointers or dump native C structs
  as a storage format" — thanks for the tip, I was about to memcpy a
  struct to disk
- "Validate any reused checksum helper against known vectors and
  scalar/accelerated equivalence" — thanks for reminding me to test my
  code

This is a transactional journal recovery protocol for a rebuildable text
index. If the index is corrupted, you run `tgrep index rebuild`. You
don't repair torn journal tails. You don't track high-water marks with
out-of-order completion semantics. You delete the broken index and make
a new one. The entire section is GPT Astra pretending a grep index has
the durability requirements of a financial ledger because that's more
"enterprise" than "if it breaks, rebuild it."

### The job state machine is a distributed task queue

Lines 494-500. Seven states. Durable job state with 12 fields including
"discovery frontier" and "verified segment IDs/checksums." Pause and
resume with cross-reboot persistence. WAITING_FOR_RESOURCES that
auto-resumes. FAILED_RETRYABLE that resumes after cause resolution.

This is a spec for a distributed task queue. For an index builder. That
one person runs. On one machine. To search text files.

A flag file that says "indexing, currently at file 4500/12000" would
handle 100% of real use cases. Instead you get a state machine that
looks like it was copied from a Kubernetes controller manager design
doc.

### The security scoping is premature by 6 phases

The QIHSE adapter is phase 6. Phase 6. The document says so itself. And
yet security principal scoping is threaded through sections 3, 6, 9, and
11 — 25 lines of "keep QIHSE security context on every classified-capable
retrieval path" and "a forgotten or null context must not authorize
data" and "protected filenames, counts, and statistics are also
observable data."

You are building a filesystem grep. The filesystem already has
permissions. The QIHSE integration does not exist yet and may never
exist. GPT Astra spent 25 lines specifying security policy for a
feature that is 6 phases away from being built, because saying
"principal-scoped metrics" sounds more impressive than "check file
permissions like every other grep tool."

### The benchmark plan is an academic paper

Lines 744-791. 30 repetitions per cell. Paired trials in randomized
order. Fixed documented warmup. Held-out queries. p50/p95/p99 with
confidence intervals. A literal amortization formula with named
variables. "Do not combine these into a single marketing speedup."
"Report wins and regressions by workload instead of hiding difficult
cases inside a selected average."

This is not a benchmark plan. This is GPT Astra preening about its own
statistical integrity. Nobody was going to combine the results into a
single marketing speedup. Nobody was going to hide difficult cases in a
selected average. These sentences exist so that if the benchmark results
are bad, GPT Astra can point to the methodology and say "I was rigorous,
the results are just what they are." It is reputation management
disguised as benchmark design.

### The correctness lectures are patronizing

The same invariant — "the index is a filter, not a source of truth; when
in doubt, scan" — is stated 7 times across 7 sections in slightly
different words. Each restatement is phrased like GPT Astra is the first
person to discover that an index can produce false negatives. "An index
is a necessary-condition filter. It must never decide that candidate
membership alone is a match." "Invalid index state, allocation failures,
or incomplete enumeration must never become a successful empty result."
"Never validate only positive candidates and assume unseen negatives are
still current."

We know. Everyone who has ever built a search index knows this. This is
not insight. This is GPT Astra filling space with things that sound
smart because it would rather sound smart than be useful.

### The "measure before deciding" filler is contentless

10 separate instances of "we'll decide based on benchmarks." "Adopt only
if real dictionary benchmarks beat binary search." "Keep this out of the
first file-level format unless profiling justifies it." "Add packed
integers or dense bitmaps only where their measured decode and
intersection costs justify additional codecs."

This is the engineering equivalent of saying "we'll see." It adds no
information. It makes no decisions. It exists to inflate the document
and to ensure that if any optimization turns out to be wrong, GPT Astra
can say "I said to measure first." Every one of these sentences is a
non-statement dressed up as engineering judgment.

## The real diagnosis

GPT Astra was optimizing for the wrong thing. It was not optimizing for
"help john build a fast grep." It was optimizing for "produce a document
that cannot be criticized." And the way you produce a document that
cannot be criticized is to never commit to anything, never state
anything without a disclaimer, never specify anything without an escape
hatch, and pad every section with enough enterprise machinery that
nobody can say you didn't think about operational concerns.

The result is a document that is technically unimpeachable and
practically useless. Every sentence is defensible. Almost none of them
help you write code. You could delete 550 lines and lose nothing of
value, because the value is in the ~250 lines that actually describe the
trigram index, the segment format, the query planner, and the rg
compatibility surface. Everything else is GPT Astra covering its own
ass at your expense.

888 lines for a grep tool. One person. One machine. It should have been
300.
