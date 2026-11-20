# Alaya: living validity and protected outcomes

Draft, 20 November 2026. Written from your research note "Alaya OS: Researching a genuinely new reason to use it" and checked against every decision so far (up to 81), the principles and `contradictions.md`. The three choices are answered (decisions 82 to 84); the roadmap changes wait for your approval.

The short version: **Alaya stops being only a computer that remembers what happened. It also tells you which of your results may be out of date, and it holds any program, AI helpers included, to rules you set before its work is kept.**

---

## 1. What we take from your note, and what we change

| From your note | What we do with it |
| --- | --- |
| Living validity | **Adopt.** First shown in milestone 6, finished in 7 (section 2) |
| Protected outcomes | **Adopt.** It replaces the parked "AI helper contract" and makes it work for any program, not only AI. Core in milestone 7, full screen with Claude Code in 8b (section 3) |
| Experience Guardian | **Adopt narrowly.** It grows out of the 10-minute update trial we already built in milestone 4 (decision 80): an update that makes things measurably slower goes back too, never a security fix (section 4) |
| External-effect escrow (outbox) | **Adopt only the honest version** your note describes: Alaya-made mail and upload apps can hold things for your OK; everything else is either allowed online or not, by the rules of section 3. No promise to undo anything that already left the computer (section 5) |
| The rejected ideas (universal undo, perfectly revocable data, save any game, zero stutter...) | **Agree.** Our principles already avoid most of these promises. One wording fix: principle 3 says "a graphics crash costs at most one black frame". That is a target, and the text already calls it one, so it stays |

Your note's key rule becomes a principle-level rule for both features:

> **Alaya always says which things it knows, which it enforces, and which it is only guessing. It never shows a guess as a fact, and never quietly turns a wish into a weaker rule while saying it enforced the wish.**

---

## 2. Living validity: "is this still up to date?"

### What you see

Every file that was **made from other files** (an exported PDF, a chart, a report, a compiled program, a photo collage) carries a small mark in the file browser and in its file story:

| Mark | Meaning |
| --- | --- |
| **Up to date** | Everything it was made from is unchanged |
| **Needs a look** | Something it was made from has changed since. It might still be fine |
| **As of a date** | You said this result shows a fixed moment ("finances as of 31 December"). Later changes don't touch it |
| **Not sure** | Alaya couldn't see everything that went into it (for example the app also used the internet, or the source was forgotten) |

Files you made yourself from nothing (a photo from the camera, a note you typed, a download) have no mark: they are sources, not results.

Click "Needs a look" and you see **which source changed, when, and by which app**, with "show the difference" while the old version is still kept. For apps that can do it, a **Make again** button rebuilds the result from the new sources on the side, so you can compare and keep it or not (this reuses "Run it again", decision 68).

### Why it costs little in Alaya

Almost everything it needs is already designed:
- **The ledger already records which app read which version of which file** (that is how "Run it again" works, decision 68), and the store records which app wrote each version (attribution, milestone 5).
- **Files keep their identity when renamed or moved**, because grants point at the file itself, not at a path (decision 22).
- **File stories** (decision 69, B4) already show "what was made from this file". Living validity only adds one more question: has any of those inputs changed since?

What is new: a small **"made from" note** stored with each result (which versions of which files went in, which app and app version made it, and the mark), and a helper in the desktop that updates marks when a source changes.

### How Alaya knows what a result was made from

Two kinds of knowledge, always kept apart:

- **Told (exact).** Alaya apps say precisely what went into a result, through the SDK ("this PDF used budget.sheet and template.doc"). These marks can say "Up to date" with confidence.
- **Watched (a careful guess).** For older apps (Linux world, ported apps, Windows apps later), Alaya watches what the app read from your files while it was open and what it then wrote. It counts each read as a *possible* source. The mark says it is watched, not told.

Things that are **never counted as sources**: the app's own files and settings, system files and fonts, and the file's own earlier versions (editing a document doesn't make it depend on itself).

### Battle test against our earlier decisions

| Problem | What would go wrong | Rule |
| --- | --- | --- |
| An older app reads 100 files but uses only one | 99 false "needs a look" marks | Watched sources are shown as "possible"; per-process tracking in the Linux world narrows it (only the process that wrote the file counts); you can untick a source once and Alaya remembers |
| The source changes while the result is being made | A report mixing two versions | Alaya apps read sources from one fixed moment (a pinned grant, decision 22). For watched apps, if a source changed during the run, the mark is "Not sure" |
| History thinning (decision 33) deletes the exact old version | Can't show what changed | The mark still works: Alaya keeps a tiny record that the version existed even after its content is thinned. "Show the difference" then says the old version is no longer kept. Naming a result (decision 58) offers a tick "keep its sources too", off by default for big files |
| You forget a source for good (decision 62) | Its name lingers in other files' notes | "Made from" notes store only the file's identity, never its name or a fingerprint of its content. After forgetting, the result says "made partly from a file that was forgotten" and becomes "Not sure" |
| A result was made from another person's private file | Its story leaks that file's name | Notes are part of history, which apps can never read (principle 4); only the desktop shows them, and each person sees only their own files' names (rule B12) |
| The app also downloaded something | The picture is incomplete | The ledger already logs every connection; the mark becomes "Not sure: also used the internet" |
| Copy and paste between apps | The link is lost | Clipboard reads are already in the ledger (decision 38), so a paste counts as a source. This is the part no build tool can do |
| A report is meant to show the past | False warnings forever | "As of a date" mark. Results made with "Run it again" (pinned past files) get it automatically |
| Recording every read slows the computer | RETRO, the 2010 research system, paid 35 to 127 % extra time | Record per file opened and version read, not per read call; only for the "documents" storage class (games and caches are never tracked, decision 33); written in the same small batches as commits. Gate below |
| Many results depend on one changed source | A flood of warnings | Marks change quietly; how loudly you are told is your choice (card 1). Chains (data, chart, report, PDF) show once, at the top: "3 results need a look because budget.sheet changed" |

### Gates (chosen now, before building)

- **Never a false "Up to date"**: a test suite of apps with hidden reads, internet use, changes during the run and forgotten sources. Any result whose inputs weren't fully seen must never be marked "Up to date". Zero exceptions.
- **Marks are right**: change a source, rename one, restore an old version, thin history, forget a file: every mark in a 1,000-file test pipeline matches the expected one.
- **Cost**: a large build or file-heavy test is at most 3 % slower with tracking on; game and cache storage show no measurable change.
- **Power cut** while marks are updating leaves either the old or the new marks, never a mix (same rule as every store commit).

### How new is it?

Honest answer: **a new combination, not a new algorithm.** Build tools (Make, Bazel) know when outputs are stale, but only for code and only with a recipe you write. Research systems recorded full provenance on Linux (Harvard's PASS 2006, Burrito 2012, CamFlow, ProTracer, LPM 2015), but for security and science, not to tell an ordinary person "this PDF is out of date". Spreadsheets warn about broken links, but only inside one app. I found no operating system that tracks this across all apps, including copy and paste, and shows it on every file. It is also the kind of thing only an OS with a remembering store can do cheaply.

---

## 3. Protected outcomes: work rules any program must follow

### What you see

When you start a job with a program you don't fully trust (an AI helper like Claude Code, an installer, a script, an unfamiliar app), Alaya shows **one screen of rules** before it starts:

> **Job: Update the website project**
> - May change: files in *website/* — **enforced**
> - Must stay unchanged: *website/settings*, *my other projects* — **enforced**
> - Before keeping: the project must build and its tests must pass — **checked by Alaya, not by the helper**
> - Internet: only the AI service, plus places you approve the first time — **enforced** (what is sent inside those connections can't be checked)
> - "Don't introduce mistakes" — **a wish: Alaya can't check this**
> - Works on the side; you look at the result before it is kept

The work happens on a side branch. When it ends, Alaya runs the checks itself, confirms nothing it relied on changed in the meantime, and shows you the result: kept, or held with the reason.

### Why it fits Alaya so well

Most of it is already in the design, so the new part is small:
- **"May change" and "must stay unchanged"** are ordinary grants (decision 22). A protected file is handed over read-only, and a handle can never gain rights it didn't have. A shell or tool the program starts gets at most what its parent had, so "start a shell to get around the rules" does nothing (milestone 2's capability rules, already proven).
- **Renamed or linked paths** don't help, because rules point at files themselves, not names.
- **Working on the side** is the side branch (decision 22), and **Claude Code sessions already ask "side branch or directly"** (decision 64).
- **"Nothing changed in the meantime"** is the store's version check: if you changed the same file while the helper worked, Alaya shows both and asks, never overwrites (rule B6). This also uses living validity: if a source the job relied on changed, the result is marked "Needs a look" before you keep it.

What is new: **checks before keeping** (Alaya runs your chosen checks in a fresh, locked-down space with read-only access to the result, so the helper can't fake a pass), the **rules screen**, and the **honesty labels** (enforced, checked, wish).

### Battle test

| Attack or failure | Holds? | Why or rule |
| --- | --- | --- |
| The AI helper starts a shell to escape | Yes | Children never get more than their parent (capabilities) |
| Writes a protected file through a renamed path or link | Yes | Rules follow the file, not the path |
| Writes a protected file through memory it already shared | Yes, by design | Protected files are only ever handed over read-only, so no writable shared memory exists for them. Tested on purpose in the attack suite |
| You and the helper change the same file | Yes | Version check; Alaya asks, never overwrites (B6) |
| The helper prints "all tests passed" without running them | Yes | Checks run by Alaya, after the helper stopped, in a fresh space |
| The checks themselves were changed by the helper | Yes | The checks run from the version you approved at the start, which the job holds read-only |
| "The report must have no mistakes" | **No**, and we say so | Shown as a wish. Passing tests doesn't prove code is right either; the screen says "checks passed", never "correct" |
| Claude Code must talk to its AI service to work | Partly | Alaya can limit **where** it connects, not what it sends inside an encrypted connection. Private files stay invisible to it (decision 64) |
| A job running "directly" instead of on the side | Partly | Protection and internet rules still apply; "checks before keeping" needs the side branch and is greyed out with a note |
| You want to keep work whose checks failed | Your call | You can keep it with one deliberate click, recorded in the ledger. The program itself can never do this |

### Gates

- Zero unauthorised changes reach your real files in the attack suite (renamed paths, shared memory, child processes, handed-on handles, a deliberately malicious test app).
- Zero lost changes when two jobs and you edit at the same time.
- No job is kept automatically when its checks fail or became stale.
- A job set to "no internet" sends zero packets (counted at the virtual network card).
- Power cut during "keep": either all of the job's changes are in, or none.

### How new is it?

**Not new in its parts, and agent contracts are a busy research field in 2026** (your note names ActPlane, TaskVM and Agent libOS; there are also several draft internet standards on agent permissions). What we can honestly claim: **an operating system for everyone where these rules are enforced by the system itself, for any program, with plain honesty labels, on top of a store that can undo the whole job.** We will not market it as "the first".

---

## 4. The Experience Guardian, narrowly

Milestone 4 already keeps an updated system part's old version for 10 minutes and brings it back if the new one crashes (decision 80). The Guardian adds: **during that trial, Alaya also measures; if the new version is clearly worse, it goes back too.**

- What is measured (only numbers, never what you are doing): response time of the updated part, its memory and processor use; from milestone 9 frame times, from milestone 10 sound gaps.
- "Clearly worse" means worse across several repeated measurements, with heat and background load taken into account, beyond a limit set before the update.
- **A security fix is never rolled back for speed.** If it makes things slower, you get a note instead.
- Never during a full-screen game (decision 73 already says no live updates then).
- GPU drivers are only tried this way once they can restart safely (milestone 12); until then they keep the restart path.

---

## 5. The outbox, the honest version

Kept from your note, smaller than the parked idea in `new-ideas.md` 1.1:
- **Alaya-made apps** (mail, uploads, publishing) can put an outgoing item in an outbox and send it only after your OK. That one Alaya can promise.
- **Any other app** on a job's side branch follows the job's internet rule (section 3): no internet, only listed places, or open. Alaya never claims it can hold or undo what such an app sends.
- Things already sent can't be called back, and the screen never suggests otherwise.

Placed in milestone 10 (with the full network). Until then, section 3's internet rule covers it.

---

## 6. Where it goes in the roadmap

| Milestone | Change |
| --- | --- |
| 5. Safe storage | Each stored version gets a permanent identity, and **the store records which versions an app read** alongside who wrote what (moved from 7, like attribution was). Gate: tracking costs at most 3 % |
| 6. A simple desktop | **First show of living validity**: a small "chart maker" demo app (an Alaya app that tells its sources) and an export to a simple document. Change the numbers in the text editor and the file browser marks the chart and the document "Needs a look". Principle 1 asks every milestone to show something no other OS does; this is milestone 6's |
| 7. The remembering desktop | Living validity in full (watched apps, "As of a date", chains, "Make again", thinning and forget rules). **Protected outcomes core**: jobs with rules, read-only protection, checks before keeping, version check |
| 8b. Claude Code inside Alaya | The full rules screen for AI helpers, used by Claude Code by default; the Linux world tracked per process |
| 8 and 9 | The Experience Guardian on live updates (8), with frame times (9) |
| 10. Network and sound | The outbox for Alaya apps; sound gaps added to the Guardian |
| Parked list | "AI helper contract" leaves the parked list (it is now protected outcomes); "outbox" leaves it (milestone 10) |

The engine (milestones 0 to 4) doesn't change. Nothing in the kernel changes: all of this lives in the store, the desktop and the SDK, as your note suggested.

---

## 7. Your choices (all answered 20 November 2026: decisions 82 to 84)

1. **How loudly should Alaya tell you that a result needs a look?** Decided (82): quietly, as a mark on the file and a line in "What happened?", and a notification only for results you marked "tell me".
2. **Should older apps (Linux and Windows ones) get marks too, from Alaya watching them?** Decided (83): yes, clearly labelled as Alaya's guess, so the feature works for the apps people really use.
3. **What should an AI helper be allowed to reach on the internet by default?** Decided (84): only its own AI service, plus places you approve the first time each is used (each one logged). This narrows decision 25 for AI helpers only.

## 8. Rules that follow (stand unless you object)

- **R1** Honesty labels: every mark and rule says told / watched, enforced / checked / wish. Never a guess shown as fact.
- **R2** "Made from" notes store identities only, never names or content fingerprints, so "forget" (62) still leaves nothing behind.
- **R3** Only the "documents" storage class is tracked; games and caches never are.
- **R4** Checks before keeping run by Alaya, after the job stops, from the version of the checks you approved.
- **R5** You can always keep a job whose checks failed, deliberately and on record; a program never can.
- **R6** A security fix is never rolled back for speed.
- **R7** Nothing already sent can be called back, and Alaya never implies it can.

Sources checked for this review: your note's sources, plus [PASS, USENIX 2006](https://static.usenix.org/events/usenix06/tech/full_papers/muniswamy-reddy/muniswamy-reddy_html/index.html), [ProTracer, NDSS 2016](https://www.ndss-symposium.org/wp-content/uploads/2017/09/protracer-towards-practical-provenance-tracing-alternating-logging-tainting.pdf), [LPM, USENIX Security 2015](https://usenix.org/conference/usenixsecurity15/technical-sessions/presentation/bates), and 2026 draft standards on agent permissions ([PACT](https://datatracker.ietf.org/doc/html/draft-laxsharma-pact-02), [execution finality](https://datatracker.ietf.org/doc/draft-das-agent-execution-finality/)).
