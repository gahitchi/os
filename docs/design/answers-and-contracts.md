# Alaya: answers instead of data, and app promises that survive updates

Draft, 20 November 2026. Written from your third review, "Alaya OS: Where else can we innovate?", and checked against every decision so far (up to 87), the principles, `contradictions.md`, `commitments.md` and `continuity.md`. Both choices are answered (decisions 88 and 89); the roadmap changes wait for your approval.

The short version: **an app can get the answer it needs without getting your data, and an app update can never quietly give itself more access.** The two bigger ideas in the review, one shared home for your data that any app can use, and handing work to your other computer, are kept for later so the roadmap doesn't grow out of control.

---

## 1. What we take from the review

| From the review | What we do with it |
| --- | --- |
| Answer capabilities (an app gets a result, not the data) | **Adopt.** It is a new kind of grant next to the ones Alaya already has. Data answers come with apps in milestone 8, and answers from the camera and microphone come in milestone 11 (section 2) |
| Behavioral software contracts (an update can't silently get more access) | **Adopt.** It makes decision 37 stronger, and the update trial (80) also notes what the new version tried to do. Milestone 8 (section 3) |
| Personal state plane (your data lives in Alaya, so apps can be swapped) | **Keep for later**, as a small, opt-in version (choice 2) |
| Portable authority (send a job to your other computer with the same limits) | **Keep for later**, after the network (milestone 10). It joins "your machines as one computer" (B7) on the parked list |
| Rejected: regenerating files instead of storing them, calling data back after it was sent, moving running apps between computers, exact battery bills, undoing all network actions | **Agree.** None of these are promised. One small idea is kept: results that can be made again exactly (living validity's "Make again") could use cheaper history space later |

---

## 2. Answers instead of data

### What you see

When an app asks for something, Alaya can offer a smaller choice than "give it the whole thing":

| The app wants to | Today | On Alaya |
| --- | --- | --- |
| Use a photo as your profile picture | Your whole photo, with where and when it was taken | A resized copy, with location and date removed |
| Show your total spending this month | Your whole bank export | One number: the total |
| Read a QR code | Your camera, live | The text in the code, nothing else |
| Know if a game controller is connected | Access to the device | Yes or no |
| Check that you are over 18 | Your date of birth | Yes or no |

The permission screen shows it plainly: *"Photo app gets: a resized copy of 1 photo, without location"*.

### How it works in Alaya

- An **answer grant** is a grant (decision 22) that doesn't open the file or device. It lets the app ask one fixed question about it.
- A small trusted **answer helper** reads the real data and sends back only the answer. The app never holds the file or the device.
- Questions come from a **fixed list** that Alaya ships (resize a photo, add up a column, read a QR code, yes or no for a device). An app can never send its own code to run on your data.
- Every answer is written in the ledger, so "What happened?" shows it: "Budget app asked for this month's total (1 number)".
- Apps that really need the whole thing, such as a photo editor, still get an ordinary grant when you hand them the file. Answers are an extra option, not a replacement.

### Battle test

| Attack or problem | Holds? | Rule |
| --- | --- | --- |
| The app tries to read the file directly | Yes | It holds no grant to the file (milestone 2's proven rules) |
| The app asks "is it over 100? over 200?..." to rebuild the number | Yes, with limits | Each answer grant has a small number of questions it can ask (for example 10 a day). The yes-or-no questions only take fixed limits (18, not any number), and running out is shown to you |
| The answer itself is private (a total from a private file) | Yes | The private mark travels with the answer (decision 27). Before the app goes online, it needs your OK, as with any private file |
| A bad app attacks the answer helper | Reduced | The helpers are small separate programs in safe Rust, fuzzed around the clock, and each one sees only the file it is answering about. If one crashes, it restarts like a driver (78, 79) |
| Timing gives away information (how long the answer took) | Partly | Answers about private files are sent after a fixed delay. Other timing leaks are studied per helper, and we don't promise more than that |
| A website won't accept a "yes" instead of your birth date | No | That depends on the website. Alaya can't fix it alone and won't pretend otherwise |
| The camera answer helper itself sees the camera | Accepted | It is part of the trusted system, the camera light is on (60), and it keeps no frames |

### Gates (chosen now)

- In the attack suite, an app holding only answer grants never gets a single byte of the original data, whether directly, through a program it starts, or by talking to other programs.
- Rebuilding a hidden number by asking questions fails within the question limit, every time.
- Every answer shows up in "What happened?".

### How new is it?

**Its parts exist.** Phones have photo pickers that hand over one photo, the web has standards for "over 18" checks without a birth date (W3C), and privacy research computes on hidden data. What we found no desktop system doing is **one general answer system, run by the OS, that covers your files, your devices and your apps the same way.** That is the claim.

---

## 3. App promises that survive updates

### What you see

When an app updates and the new version wants more than the old one had, Alaya shows the difference before anything changes:

> **Music player 2.0** wants more than version 1 had:
> - **New:** the microphone
> - **New:** connections to 2 more internet places
> Everything else stays as you allowed it.
> **[Update, keep the old limits]** **[Allow the new things]** **[Don't update]**

### How it works in Alaya

- An app's install list (decision 37) becomes its **promise**. That is everything it may touch, as you approved it, and you can untick items (60).
- **An update can never widen the promise by itself.** Signed by the same developer or not, the new version starts with exactly the old version's limits.
- New asks are shown as a difference (decision 88: you are asked when the app first tries to use them).
- During the update's 10-minute trial (decision 80), Alaya also notes what the new version **tried** to do: a new internet place it tried to reach, a file it tried to open. That goes into the update note. This is evidence, not proof, and the promise is still the real wall.
- **Security updates are never held back** because an app wants more. The update installs with the old limits, and only the new asks wait for you.

### Battle test

| The new version tries to | Result |
| --- | --- |
| Read a folder it never had | Blocked: not in the promise |
| Connect to a new internet place | Allowed only under the network rules: decision 25 (open), or 84 for AI helpers. Either way it is listed in the update note. If the old promise named its internet places, a new one is blocked |
| Change a file it doesn't own | Blocked, recorded |
| Wait until after the trial, then do something bad | Not caught by the trial, which is why the promise, not the trial, is the wall. The ledger still records it |
| Be a reasonable new feature | You approve it in one click, and it is recorded and can be undone |

### How new is it?

**Not new in its parts.** Phone and Flatpak permissions exist, and research has documented apps quietly gaining permissions over versions. Alaya's version is **a promise that can never grow on its own, tied to the update trial, the ledger and one-click undo of the whole update (73).**

---

## 4. Kept for later

**A shared home for your data (personal state plane).** This means notes, calendar entries and contacts stored as Alaya objects, which any compatible app can open, so you can swap apps without exporting. It is a great idea and a big bet: it only works if apps support it. Solid and local-first research (Automerge) show how hard it is. The honest small version uses **one or two data types and two apps written separately**, and nothing is ever lost when an app doesn't understand a field. It waits until Alaya runs everyday apps (decision 89).

**Handing work to your other computer (portable authority).** This means sending a long job (a build, a render) to another Alaya machine with exactly the same limits, plus a receipt. Revoking it while the other machine is offline can't be instant, so it would need short leases. It needs the network (10) and machine identity first, so it goes on the parked list next to B7.

---

## 5. Where it goes in the roadmap

| Milestone | Change |
| --- | --- |
| 8. Apps and the old world | App promises that can't grow on their own, the update difference screen, and the trial noting what an update tried. The answer system with its first data answers: photo for a profile picture, a column total, "is this device connected" |
| 10. Network and sound | Microphone answers (for example "is someone speaking", not the sound itself) |
| 11. The laptop's everyday devices | Camera answers (QR code), once the real camera is reachable |
| Parked list | The shared home for your data (unless choice 2 says otherwise), and handing work to your other computer |

The kernel doesn't change. Answer helpers are ordinary programs that use the grants we already have.

## 6. Your choices

1. **When an update asks for more access, what should happen?** Decided (88): **update straight away with the old limits, and ask about the new things only when the app first tries to use them.** That way security fixes never wait. The other choice is to hold the whole update until you decide.
2. **When should we try the shared home for your data?** Decided (89): **later, after Alaya can run everyday apps** (with milestone 16, on the parked list), so the roadmap doesn't balloon. The other choice is a small version in milestone 8, with notes as the first shared type.

## 7. Rules that follow (stand unless you object)

- **R8** Answer questions come only from Alaya's fixed list; an app never runs its own code on your data.
- **R9** Every answer grant has a question limit, and every answer is in the ledger.
- **R10** An answer from a private file is private too (decision 27).
- **R11** An update never widens an app's promise by itself, and a security update is never held back because of new asks.
- **R12** What an update tried during its trial is shown as evidence, never as proof that it is safe.
