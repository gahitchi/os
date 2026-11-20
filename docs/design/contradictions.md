# Alaya: every contradiction in the concept

19 November 2026. A check of everything together: the 8 principles, decisions 1 to 70, the design files (alaya-design, battle-test, timeline, linux-compat-options, kernel-interface, test-plan, roadmap), `new-ideas.md`, `concept-analysis.md`, and what is built and merged in the repository (milestones 0 to 3, including `docs/threads-and-messages.md`).

Each contradiction is one of three kinds:
- **Needs you**: a real choice between two things you want. These are on decision cards.
- **Rule**: the answer follows from decisions you already made; I propose a rule that stands unless you object.
- **Wording**: the design is fine, the text says something slightly different. Fixed when the roadmap is rebuilt.

---

## A. Needs you (5 decision cards, all answered: decisions 71 to 75, each as recommended)

### A1. Tasks vs the terminal and Claude Code

- Decision 69: an app opened in a task sees that task's files, and nothing else.
- Decision 40: the terminal sees all of your files. Decision 66: Claude Code runs in the Linux world with the terminal's limits.
- So the most powerful tools ignore tasks entirely, and Claude Code opened in your "Thesis" task can read your "Flat move" contract.

Options: the terminal (and Claude Code) opened in a task sees only that task's files, with a one-click "whole tree" switch (recommended, it makes tasks mean something); or keep decision 40 as it is.

### A2. Apps that belong everywhere

- Decision 69: everything lives in a task, and paused tasks use nothing.
- Decision 46: music, downloads and chat keep running in the background with a background grant.
- A music player or chat app in a paused task would stop. Opening it in every task would mean many copies.

Options: a small "everywhere" area for apps that are not tied to one task (music, chat, downloads), which see only their own folder and what you give them (recommended); or every app belongs to exactly one task.

### A3. When updates without restart are applied

- Decision 61 (your choice): updates prepare quietly and apply at your next restart, never forced.
- Decision 70: drivers and system parts can now be updated while you work.
- Should they now apply by themselves?

Options: apply by themselves when no full-screen game is running, with a note and one-click undo (recommended; it is what "no restart" is for); or ask each time; or keep applying at the next restart only.

### A4. Admin vs other people's privacy

- Decision 59 (your choice): a classic admin can change anything, with a PIN.
- Decisions 30 and 59: admin can never read another person's files.
- But an admin who changes the login screen could capture another person's PIN. So "admin can never read your files" only holds against an honest admin.

Options: tell each person at login when the login or key parts of the system changed since their last login (recommended, light); or require every person's PIN for those changes (strong, but a family laptop could get stuck); or keep it as it is and say it plainly in the docs.

### A5. "No real boots until stable" vs milestones 11 and 12

- Your rule: Alaya is tested on the laptop only inside QEMU/KVM, with no real boots until it is completely stable.
- Roadmap milestones 11 and 12 say "Alaya boots to its desktop on the laptop" and build the laptop's real drivers, which QEMU's virtual devices can't test.

Options: hand single real devices (a USB controller, the Wi-Fi card, the NVIDIA card) to Alaya running inside KVM, so drivers are tested on real hardware while Linux stays in charge; first real boot only after all stress tests pass and you say so (recommended); or boot from a USB stick at milestone 11, never touching the laptop's disk.

---

## B. Rules that follow from your decisions (stand unless you object)

| # | Clash | Rule |
| --- | --- | --- |
| B1 | **The private mark spreads to everything** (decision 27): every app shares screen buffers with the compositor and writes through the store, so after one private file everything would be marked | The compositor, the store, the clipboard server, the input-method server and the audio server are named *trusted carriers*: they pass data without being marked. Only apps are marked |
| B2 | **"Run it again as last Tuesday" (68) vs thinning (33)**: after a day only hourly moments are kept, so the exact files an app read may be gone | Re-run is exact while those versions are kept (always within a day; named versions forever); older re-runs use the nearest kept moment and say so plainly |
| B3 | **Paused tasks "use nothing" (69) vs no disk swap (24) and older apps' own Save button (58)** | Alaya apps in a paused task save their state and close; older Windows and Linux apps stay frozen in memory (no CPU), so no unsaved work is ever lost. If memory runs low, they are asked to save first, as in decision 24 |
| B4 | **One undo (69) vs installs shared by all people (30, 37)** | Undoing an install removes the app for you only. Removing it for everyone is an admin action and needs a PIN, like any admin change |
| B5 | **One undo vs admin changes (59)** | Undoing an admin change needs the PIN too |
| B6 | **One undo vs other apps' later changes** | As with per-app undo: if something else changed the same thing since, Alaya shows both and asks; it never silently overwrites |
| B7 | **Frame deadlines (70) vs fixed priorities (11, 53, 54)** | Priorities still decide first and budgets are never exceeded. Deadlines only order threads within the same priority and help place them, so all of milestone 3's proofs still hold |
| B8 | **Quiet driver crashes (70) vs requests that can't be repeated** | Disk and input requests are sent again; network packets may be sent again (the network already allows that); graphics work cannot be, so a graphics crash still costs one black frame (principle 3 unchanged) |
| B9 | **Live updates (70) vs updates on a branch with boot fallback (28)** | A live update still installs on a branch. If the new part misbehaves, Alaya swaps the old one back live; if the machine can't start, decision 28's fallback still works |
| B10 | **Live updates vs game mode (63)** | No live update is applied while a full-screen game is running; it waits until the game closes |
| B11 | **Linux world (66) vs tasks (69)** | The Linux world follows whatever you choose for the terminal in A1 |
| B12 | **Tasks (69) vs private-file guard and "What happened?"** | Tasks never weaken the private mark; a private file in a task is still private, and "What happened?" shows each person only their own tasks |

---

## C. Wording fixes (no choice needed)

| # | Where | Says | Fix |
| --- | --- | --- | --- |
| C1 | Principle 2 headline | "Nothing gets in by default" | The network, clipboard and terminal are open by your choice. New headline: "Apps only see what you hand them, and you can see everything they did" |
| C2 | Principle 2 "Never" | "Never send anything about you anywhere without you seeing it first" | True for Alaya itself, not for apps you allowed online. Say "Alaya itself never sends…" |
| C3 | Principle 2 "Always" | "An app starts with only its own folder" | Add: "…plus the files of the task it is opened in" (decision 69) |
| C4 | Principle 4 | "See your files at any moment within a tenth of a second" | "At any kept moment" (thinning, decision 33) |
| C5 | Principle 3 | "A graphics crash costs at most one black frame" | A target until the NVIDIA driver exists (milestone 12), not yet a promise |
| C6 | Principle 3 and decision 61 | "Updates apply at your next restart" | Updated after card A3 |
| C7 | Principle 7, alaya-design §12, decision 52 | Claude Code runs on "Node.js recompiled for Alaya" | Decision 66: it runs in the Linux world |
| C8 | Decisions 42, 48, 50 | Accessibility, multiple monitors and input methods "from the first desktop" | Decision 67 moved them to milestone 7b; the toolkit still carries the accessibility tree from milestone 6 |
| C9 | `principles-design.md` comparison tables | Windows "often a blue screen" on a driver crash; no auto-save on macOS; no history | Windows recovers from graphics driver crashes since 2006; macOS has auto-save and versions since 2011 and hourly local snapshots. Correct those rows |
| C10 | `docs/threads-and-messages.md` (built, milestone 3) | "the placement service using Intel's hardware feedback … in milestone 4" | Decision 54 put placement in the kernel. Milestone 4 only adds a small helper that reads Intel's feedback and sets each budget's core type |
| C11 | Public repository `docs/design/` | Stopped at about decision 52 and the old roadmap | Fixed 20 November 2026: decisions 53 to 75, the analysis, the new ideas, this file and the new roadmap were added to the repository |
| C12 | `alaya-design.md` | Names the reference laptop in private files | Fine privately; it must say "the reference laptop" before any public copy (your rule) |

---

## D. Checked and consistent

- The clash order (safe data, steady performance, games, privacy) is applied correctly in every later decision, including 66 to 70.
- Milestones 0 to 3 match decisions 5, 7 to 11, 26, 51, 53 to 56: the kernel allocates nothing, 10 calls, sporadic refills, lent time, kernel placement, address spaces. Frame deadlines (70) need no new kernel call; they travel inside the existing scheduling context.
- History and grants (22), tasks (69), "What happened?" (68) and file stories (69) all read from the same store and ledger; none needs a second record.
- Games (6, 21, 63) and the new ideas: none of the new ideas runs during full-screen games or touches the frame path, except frame deadlines, which exist to help it.

## E. Living validity and protected outcomes (20 November 2026)

Checked against every decision up to 81 in `commitments.md` (battle-test tables in sections 2 and 3). No clash with a decision; rules R1 to R7 there settle the meeting points: thinning (33), forget (62), private files and other people (B12, principle 4), clipboard (38), side branch or directly (64), the open network (25, narrowed for AI helpers only if you choose so), live updates (73, 80).
