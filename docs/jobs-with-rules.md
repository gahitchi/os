# Jobs with rules, "Run it again" and living validity (milestone 7, step 3)

Step 3 of milestone 7 lets you give work to a program you don't fully
trust, without trusting it: Alaya, not the program, enforces the rules
and checks the result. It also lets an app run again on exactly what it
read at a past moment, and finishes living validity ("is this still up
to date?"). The plan is `milestone-7-plan.md` (parts 12 to 14); the
choices made while the project's owner was away are decisions 117 to 120
in [`design/decisions.md`](design/decisions.md), marked "to review".

## What a person sees

- **A job, with one screen of rules.** A program that wants to change
  your files asks for a job (the tidy-up helper's "Ask for a job"). Alaya
  shows your folders; you pick **the one folder it may change**. Then the
  rules, each with an honest label:
  - May change: files in that folder (**enforced**).
  - Must stay unchanged: the folder's `checklist.txt` and your private
    files in it (**enforced**). Private files are not even visible to it.
  - Everything else of yours: not there for it (**enforced**).
  - Before keeping: nothing lost, nothing emptied, and every line of
    `checklist.txt` as it is now (**checked by Alaya**).
  - Internet: none (Alaya has no network yet; **enforced**).
  - "Do a good job": **a wish**, which Alaya can't check.
- **It works on a side copy.** Nothing reaches your files while it works.
- **When it is done, Alaya checks.** The result screen lists each check
  ("Passed: Nothing lost", "Did not pass: Nothing emptied (day-1.txt)"),
  and "Needs a look" for any file the job read that you changed since.
  - Every check passed: **Keep** puts the work in your files in one step.
  - A check failed: the work is **held**. Enter does nothing; **Keep
    anyway** (a click, or K) is your call, and the history says so.
  - You changed some of the same files: Alaya asks, and your version
    stays for those (rule B6).
  - **T** throws the work away; **Later** keeps it waiting on the side.
- **Run it again** ("What happened?", A or the button): an app opens
  again on exactly the documents it had read at that moment, each at the
  version it read, read only. When history no longer keeps that moment,
  the nearest kept one stands in, and Alaya says so.
- **Copy and paste carry the mark.** In the text editor, Ctrl+C copies
  the line you are on and Ctrl+V pastes. When the line came from a saved
  document, the clipboard knows exactly which version, so the document
  you paste into is noted as made from it: "up to date" while the source
  is unchanged, "needs a look" when it changes. A paste from a private
  document marks the app that receives it, like reading it would.
- **Show the difference** (the file browser, S on a result that needs a
  look): what changed in its source since ("was "pears 4", now "pears
  6"").
- **Make again** (the file browser, M): the app that made the result
  makes it again from its sources as they are now. The chart maker can;
  for other apps Alaya says to open it and save again.

## How it works

- **Jobs** (`store/src/jobs.rs`) sit on the store's side branches. A
  job's rules (the folder, what must stay unchanged, the checks, the
  checklist) are kept with your app files, keyed by its side branch, so
  a job survives a restart. The store server sends every document request
  of the job's app to the side branch, after asking the store whether the
  file is inside the folder; rules follow files, not names, so renaming or
  moving does not help. Once the app says it is done, it can change
  nothing more.
- **Checks run by Alaya**, never by the app: on the side copy, read only,
  after the app stopped. The checklist is read from the version you
  approved (the moment the job started), so an app that edits it changes
  nothing. Lines are `has <path>`, `says <path> <words>`, `not <path>
  <words>` and `under <path> <bytes>`; a line Alaya cannot read fails, so
  a typo never passes. The checks that are enforced anyway (nothing
  outside the folder, nothing protected changed) are checked again.
- **Keeping is one commit** (a power cut leaves all of it or none), and
  only the desktop can ask for it; an app asking is refused. Work kept
  although its checks failed is recorded with the ANYWAY mark.
- **Run it again** (`store/src/again.rs`): the newest read of each
  document by that app up to that moment, and the kept moment that holds
  that version. The store server serves the app from those moments until
  its last window closes; writes are refused.
- **The clipboard** is kept by the store server (a trusted carrier, rule
  B1), with the exact version a copy came from. A paste notes "this app
  was handed part of this version", which `made_from` accepts as a
  source for living validity. "Show the difference" reads the source as
  the result used it (from history) and as it is now; the lines are
  compared simply (`ui/src/diff.rs`).

## How it is tested

On the host: `cargo test -p alaya-store --test jobs` (a job changes only
its folder; failed checks hold the work until it is kept anyway, on
record; a file changed since makes the work stale; a done job changes
nothing and survives a restart; bad rules start nothing; two jobs and you
at once lose nothing), `--test again` (exact versions, other apps' reads,
forgotten files, the nearest kept moment after thinning), `--test made`
(a pasted part carries the mark and shows the difference), and the
difference itself in `alaya-ui`.

Inside Alaya (`cargo xtask test`), checks 93 to 96:

| Check | What it shows |
| --- | --- |
| 93 | The tidy-up helper asks for a job; the rules screen is shown; it tidies only its folder, on a side copy; nothing reaches your files until Alaya's checks pass and you keep it; the history shows its kept work |
| 94 | A hostile helper tries 14 ways around the rules (your other files, guessed file numbers, the folder itself, moving files out, the checklist, a private file, keeping its own work, a second job, changing after "done"): all refused; its damage inside the folder is held by Alaya's checks; Enter never keeps it; T throws it away; your files are untouched |
| 95 | Run it again: the chart maker sees exactly the numbers it read then, cannot change them, sees nothing else, and sees today's numbers again once it closes |
| 96 | A pasted line carries its source's mark ("up to date", then "needs a look"), the difference shows what changed, a paste from a private file marks the app that receives it, and the chart maker makes its picture again on request |

## What is not there yet

- One folder per job, chosen from the top of your documents.
- "Must stay unchanged" is the checklist and private files; choosing
  other files on the rules screen comes later.
- Checks are Alaya's built-in ones and the checklist; "build and run the
  tests" needs programs to run them in a locked-down space, which comes
  with app installs (milestone 8) and Claude Code (8b).
- Copy takes the line you are on (the editor has no selection yet).
- Only the chart maker can make again by itself.
