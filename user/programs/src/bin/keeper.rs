// SPDX-License-Identifier: GPL-3.0-only
//! The driver keeper: looks after the disk driver so apps never see it
//! crash (decisions 70, 78, 79, 80). See the crate documentation for the
//! rules; this file is the one loop that carries them out.
//!
//! Everything happens on one thread, woken by one of four things: an app's
//! request, a control request from the first program, a fault of a driver
//! (crash), or a signal (the driver finished jobs, the clock ticked, or the
//! driver parked itself for an update).

#![no_std]
#![no_main]

use alaya_programs::driver as d;
use alaya_programs::keeper as k;
use alaya_programs::{JobPage, OP_READ, OP_WRITE, SECTOR, load, registry, run, status, store};
use alaya_sys::abi::{FAULT_LABEL, StartInfo, call, iomap, map, start};
use alaya_sys::program::{self, Parent, Program, Slots, Spec};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{At, Caps, Kind, Msg, MsgInfo, rights, say};

alaya_sys::entry!(main);

/// Parent slots set aside for building one driver instance.
const SPAN: u64 = 48;
/// Memory each driver instance is made from.
const DRIVER_BITS: u8 = 21;
const DRIVER_PRIORITY: u64 = 170;
/// Reply objects: one per request the keeper holds at once.
const REPLIES: usize = 80;
/// Requests waiting for a free job slot.
const QUEUE: usize = 40;

#[derive(Clone, Copy)]
struct Instance {
    ut: u64,
    base: u64,
    sc: u64,
    fault: u64,
    prog: Option<Program>,
    image: usize,
}

#[derive(Clone, Copy, Default)]
struct Request {
    reply: u64,
    label: u64,
    sector: u64,
    seed: u64,
}

#[derive(Clone, Copy)]
struct Update {
    reply: u64,
    image: usize,
    trial_ms: u64,
    flags: u64,
    /// The new instance was started; waiting for it to run.
    started: bool,
}

struct Keeper {
    info: &'static StartInfo,
    mem: Mem,
    ep: u64,
    me: u64,
    dntfn: u64,
    park: u64,
    irq: u64,
    io: u64,
    page_job: u64,
    page_handover: u64,
    page_ring: u64,
    page_header: u64,
    config: u64,
    dev: [(u64, u64); 12],
    ndev: usize,
    jobs: &'static JobPage,
    data: u64,
    images: [(u64, u64); 2],
    scratch: u64,
    inst: [Instance; 2],
    cur: usize,
    /// The previous version, parked, while the new one is on trial.
    old: Option<usize>,
    trial_end: u64,
    update: Option<Update>,
    seq: u64,
    slot: [Option<Request>; d::JOBS],
    queue: [Request; QUEUE],
    queued: usize,
    replies: [u64; REPLIES],
    free_replies: usize,
    off: bool,
    limit: u64,
    window: u64,
    crashes: [u64; 16],
    ncrash: usize,
    sectors: u64,
    last_finished: u64,
    progress_ms: u64,
    chaos: bool,
    chaos_min: u64,
    chaos_max: u64,
    chaos_next: u64,
    rng: u64,
    // Counters.
    restarts: u64,
    resent: u64,
    updates: u64,
    rollbacks: u64,
    hangs: u64,
    crashes_seen: u64,
    signal_driver: bool,
}

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    let mut mem = Mem::new(info);
    let (page_job, job_va) = mem.page().expect("job page");
    let (page_handover, _) = mem.page().expect("handover page");
    let ep = g + k::G_ENDPOINT;
    let io = g + k::G_IO;
    let me = mem.make(Kind::Notification, 0).expect("notification");
    alaya_sys::thread_bind(start::THREAD, me).expect("bind");
    let dntfn = mem.make(Kind::Notification, 0).expect("notification");
    let park = mem.make(Kind::Notification, 0).expect("notification");

    // Pages the device may reach: the ring, request headers and data.
    let dma = |mem: &mut Mem, iova: u64| -> (u64, u64) {
        let (p, va) = mem.page().expect("dma page");
        let c = mem.slot().expect("slot");
        alaya_sys::copy(mem.at(c), mem.at(p), rights::ALL).expect("copy");
        alaya_sys::page_iomap(c, io, iova, iomap::WRITE).expect("iomap");
        (p, va)
    };
    let (page_ring, _) = dma(&mut mem, d::IOVA_RING);
    let (page_header, _) = dma(&mut mem, d::IOVA_HEADER);
    let data = mem.window(d::DATA_PAGES);
    for i in 0..d::DATA_PAGES {
        let p = mem.make(Kind::Page, 12).expect("page");
        alaya_sys::page_map(p, start::SPACE, data + i * 4096, map::WRITE).expect("map");
        let c = mem.slot().expect("slot");
        alaya_sys::copy(mem.at(c), mem.at(p), rights::ALL).expect("copy");
        alaya_sys::page_iomap(c, io, d::IOVA_DATA + i * 4096, iomap::WRITE).expect("iomap");
    }

    let a = &info.args;
    let ndev = (a[k::ARG_DEV_PAGES] as usize).min(12);
    let mut dev = [(0, 0); 12];
    for (i, e) in dev.iter_mut().enumerate().take(ndev) {
        *e = (g + k::G_DEV + i as u64, a[k::ARG_DEV_CODES + i]);
    }
    let mut inst = [Instance {
        ut: 0,
        base: 0,
        sc: 0,
        fault: 0,
        prog: None,
        image: 0,
    }; 2];
    for (n, x) in inst.iter_mut().enumerate() {
        x.ut = mem.make(Kind::Untyped, DRIVER_BITS).expect("driver memory");
        x.sc = g + k::G_SC + n as u64;
        x.fault = mem.slot().expect("slot");
        alaya_sys::mint(
            mem.at(x.fault),
            mem.at(ep),
            rights::ALL,
            k::FAULT + n as u64,
        )
        .expect("mint");
        x.base = mem.next;
        mem.next += SPAN;
    }
    let mut replies = [0; REPLIES];
    for r in replies.iter_mut() {
        *r = mem.make(Kind::Reply, 0).expect("reply object");
    }
    let scratch = mem.window(1);
    rt::start_clock(&mut mem, me, k::TICK_BIT, 1, 180).expect("clock");

    // SAFETY: the job page was just mapped here, writable, and stays.
    let jobs = unsafe { &*(job_va as *const JobPage) };
    let mut kp = Keeper {
        info,
        mem,
        ep,
        me,
        dntfn,
        park,
        irq: g + k::G_IRQ,
        io,
        page_job,
        page_handover,
        page_ring,
        page_header,
        config: g + k::G_CONFIG,
        dev,
        ndev,
        jobs,
        data,
        images: [
            (k::IMAGES, a[k::ARG_IMAGE_SIZE]),
            (k::IMAGES + k::IMAGE_STEP, a[k::ARG_IMAGE_SIZE + 1]),
        ],
        scratch,
        inst,
        cur: 0,
        old: None,
        trial_end: 0,
        update: None,
        seq: 0,
        slot: [None; d::JOBS],
        queue: [Request::default(); QUEUE],
        queued: 0,
        replies,
        free_replies: REPLIES,
        off: false,
        limit: k::RESTART_LIMIT,
        window: k::RESTART_WINDOW_MS,
        crashes: [0; 16],
        ncrash: 0,
        sectors: 0,
        last_finished: 0,
        progress_ms: 0,
        chaos: false,
        chaos_min: 0,
        chaos_max: 0,
        chaos_next: 0,
        rng: 0x2545_f491_4f6c_dd1d,
        restarts: 0,
        resent: 0,
        updates: 0,
        rollbacks: 0,
        hangs: 0,
        crashes_seen: 0,
        signal_driver: false,
    };
    let _ = kp.io;
    // The first start points the device at memory it does not own once,
    // to show the guard stopping it.
    if let Err(e) = kp.start(0, 0, d::FLAG_PROBE) {
        say!("cannot start the driver: {e:?}");
    }
    kp.register();
    say!("looking after the disk driver");
    kp.serve()
}

impl Keeper {
    fn now(&self) -> u64 {
        rt::now_ms()
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Tell the registry where apps find the disk.
    fn register(&mut self) {
        let c = self.mem.slot().expect("slot");
        alaya_sys::mint(self.mem.at(c), self.mem.at(self.ep), rights::ALL, k::CLIENT)
            .expect("mint");
        let n = registry::name("disk");
        let m = Msg {
            info: MsgInfo::new(registry::REGISTER, 2).with_cap(),
            badge: 0,
            words: [n[0], n[1], 0, 0, 0, 0],
        };
        let r = alaya_sys::ipc_call(
            start::FIRST_GIVEN + k::G_REGISTRY,
            &m,
            Caps {
                send: c,
                ..Caps::default()
            },
        );
        if r.map(|m| m.words[0]) != Ok(0) {
            say!("the registry refused the name");
        }
    }

    /// Build and start a driver instance in slot `n`.
    fn start(&mut self, n: usize, image: usize, flags: u64) -> Result<(), alaya_sys::Error> {
        self.kill(n);
        let x = self.inst[n];
        let (addr, size) = self.images[image];
        // SAFETY: the first program mapped the images here, read only.
        let elf = unsafe { core::slice::from_raw_parts(addr as *const u8, size as usize) };
        let mut shared = [(0u64, 0u64, 0u64); 20];
        let fixed = [
            (self.page_job, d::JOB_PAGE),
            (self.page_handover, d::HANDOVER_PAGE),
            (self.page_ring, d::RING_PAGE),
            (self.page_header, d::HEADER_PAGE),
            (self.config, d::CONFIG_PAGE),
        ];
        let mut ns = 0;
        for (h, at) in fixed {
            shared[ns] = (h, at, map::WRITE);
            ns += 1;
        }
        let mut args = [0u64; 16];
        args[d::ARG_FLAGS] = flags;
        args[d::ARG_DEV_PAGES] = self.ndev as u64;
        for i in 0..self.ndev {
            let (h, code) = self.dev[i];
            shared[ns] = (h, d::bar_page(code >> 16, code & 0xffff), map::WRITE);
            ns += 1;
            args[d::ARG_DEV_CODES + i] = code;
        }
        store(&self.jobs.state, run::STARTING);
        let mut parent = Parent {
            space: start::SPACE,
            scratch: self.scratch,
            slots: Slots {
                table: start::TABLE,
                next: x.base,
                end: x.base + SPAN,
            },
        };
        let spec = Spec {
            name: if image == 0 { "blk1" } else { "blk2" },
            image: elf,
            untyped: x.ut,
            table_bits: 7,
            own_bits: 0,
            priority: DRIVER_PRIORITY,
            sc: x.sc,
            clock_sc: 0,
            fault: x.fault,
            given: &[self.dntfn, self.park, self.me, self.irq],
            shared: &shared[..ns],
            args: &args[..2 + self.ndev],
            clock_hz: self.info.clock_hz,
            cores: self.info.cores,
        };
        let p = program::start(&mut parent, &spec)?;
        self.inst[n].prog = Some(p);
        self.inst[n].image = image;
        self.progress_ms = self.now();
        Ok(())
    }

    /// End driver instance `n`, and take back all it was made of.
    fn kill(&mut self, n: usize) {
        let x = &mut self.inst[n];
        let _ = alaya_sys::revoke(At {
            table: start::TABLE,
            index: x.ut,
        });
        x.prog = None;
    }

    fn version_of(&self, n: usize) -> u64 {
        self.inst[n].image as u64 + 1
    }

    fn pending(&self) -> u64 {
        self.jobs
            .jobs
            .iter()
            .filter(|j| load(&j.posted) != load(&j.done))
            .count() as u64
    }

    // -----------------------------------------------------------------
    // The loop

    fn take_reply(&mut self) -> u64 {
        if self.free_replies == 0 {
            // Every reply object holds a waiting request; the queue limits
            // make this impossible.
            panic!("out of reply objects");
        }
        self.free_replies -= 1;
        self.replies[self.free_replies]
    }

    fn give_reply(&mut self, r: u64) {
        self.replies[self.free_replies] = r;
        self.free_replies += 1;
    }

    fn serve(&mut self) -> ! {
        let mut r = self.take_reply();
        loop {
            let got = alaya_sys::msg_call(
                call::RECV,
                self.ep,
                &Msg::default(),
                Caps {
                    reply: r,
                    ..Caps::default()
                },
            );
            match got {
                Err(_) => {
                    // The reply object is still held: use another.
                    r = self.take_reply();
                }
                Ok(m) if m.info.has(MsgInfo::SIGNAL) => self.on_signal(m.badge),
                Ok(m) => {
                    if self.on_message(&m, r) {
                        r = self.take_reply();
                    }
                }
            }
            self.harvest();
            if self.signal_driver {
                self.signal_driver = false;
                let _ = alaya_sys::signal(self.dntfn, d::JOBS_BIT);
            }
        }
    }

    /// Returns true if the reply object now holds a waiting caller.
    fn on_message(&mut self, m: &Msg, r: u64) -> bool {
        let w = m.words;
        if m.label() == FAULT_LABEL && m.badge >= k::FAULT {
            let n = (m.badge - k::FAULT) as usize;
            self.crashes_seen += 1;
            if self.crashes_seen <= 3 {
                say!(
                    "driver version {} crashed (vector {}, address {:#x}, at {:#x})",
                    self.version_of(n.min(1)),
                    w[0],
                    w[2],
                    w[3]
                );
            }
            self.on_crash(n.min(1), false);
            return false;
        }
        match (m.badge, m.label()) {
            (k::CLIENT, k::READ | k::WRITE) => {
                let req = Request {
                    reply: r,
                    label: m.label(),
                    sector: w[0],
                    seed: w[1],
                };
                if self.off {
                    let _ = alaya_sys::reply(r, &Msg::new(0, &[status::DEVICE_OFF, 0]));
                    false
                } else if self.sectors != 0 && w[0] >= self.sectors {
                    let _ = alaya_sys::reply(r, &Msg::new(0, &[status::BAD_REQUEST, 0]));
                    false
                } else if let Some(i) = self.slot.iter().position(|s| s.is_none()) {
                    self.post(i, req);
                    true
                } else if self.queued < QUEUE {
                    self.queue[self.queued] = req;
                    self.queued += 1;
                    true
                } else {
                    let _ = alaya_sys::reply(r, &Msg::new(0, &[status::BUSY, 0]));
                    false
                }
            }
            (k::CLIENT, k::SIZE) => {
                let _ = alaya_sys::reply(r, &Msg::new(0, &[load(&self.jobs.sectors)]));
                false
            }
            (k::CONTROL, k::STATS) => {
                let state = if self.off {
                    k::state::OFF
                } else if self.update.is_some() {
                    k::state::UPDATING
                } else if self.old.is_some() {
                    k::state::TRIAL
                } else {
                    k::state::RUNNING
                };
                let s = [
                    self.restarts,
                    self.resent,
                    self.updates,
                    self.rollbacks,
                    load(&self.jobs.version),
                    state,
                ];
                let _ = alaya_sys::reply(r, &Msg::new(0, &s));
                false
            }
            (k::CONTROL, k::MORE) => {
                let left = if self.old.is_some() {
                    self.trial_end.saturating_sub(self.now())
                } else {
                    0
                };
                let s = [
                    self.hangs,
                    self.crashes_seen,
                    load(&self.jobs.probe),
                    load(&self.jobs.finished),
                    left,
                ];
                let _ = alaya_sys::reply(r, &Msg::new(0, &s));
                false
            }
            (k::CONTROL, k::CHAOS) => {
                self.chaos = w[0] != 0;
                self.chaos_min = w[1].max(1);
                self.chaos_max = w[2].max(self.chaos_min + 1);
                if w[3] != 0 {
                    self.rng = w[3];
                }
                self.chaos_next = self.now() + self.chaos_min;
                let _ = alaya_sys::reply(r, &Msg::new(0, &[0]));
                false
            }
            (k::CONTROL, k::POLICY) => {
                self.limit = w[0];
                self.window = w[1];
                self.ncrash = 0;
                let _ = alaya_sys::reply(r, &Msg::new(0, &[0]));
                false
            }
            (k::CONTROL, k::TRY_AGAIN) => {
                let ok = if self.off {
                    say!("trying the disk again, as asked");
                    self.off = false;
                    self.ncrash = 0;
                    let image = self.inst[self.cur].image;
                    self.start(self.cur, image, 0).is_ok()
                } else {
                    true
                };
                let _ = alaya_sys::reply(r, &Msg::new(0, &[if ok { 0 } else { 1 }]));
                false
            }
            (k::CONTROL, k::FREEZE_NOW) => {
                self.freeze_driver();
                let _ = alaya_sys::reply(r, &Msg::new(0, &[0]));
                false
            }
            (k::CONTROL, k::CRASH_NOW) => {
                self.crash_driver();
                let _ = alaya_sys::reply(r, &Msg::new(0, &[0]));
                false
            }
            (k::CONTROL, k::UPDATE) => self.begin_update(r, w[0] as usize, w[1], w[2]),
            _ => {
                let _ = alaya_sys::reply(r, &Msg::new(0, &[status::BAD_REQUEST]));
                false
            }
        }
    }

    fn on_signal(&mut self, bits: u64) {
        if bits & k::PARKED_BIT != 0 {
            self.continue_update();
        }
        if bits & k::TICK_BIT != 0 {
            self.tick();
        }
    }

    // -----------------------------------------------------------------
    // Jobs

    fn post(&mut self, i: usize, req: Request) {
        let buf = self.data + i as u64 * SECTOR as u64;
        if req.label == k::WRITE {
            for w in 0..(SECTOR / 8) as u64 {
                let v = alaya_programs::pattern(req.seed, req.sector, w);
                // SAFETY: the data pages are mapped here, writable.
                unsafe { core::ptr::write_volatile((buf + w * 8) as *mut u64, v) };
            }
        }
        let j = &self.jobs.jobs[i];
        store(
            &j.op,
            if req.label == k::WRITE {
                OP_WRITE
            } else {
                OP_READ
            },
        );
        store(&j.sector, req.sector);
        self.seq += 1;
        store(&j.posted, self.seq);
        self.slot[i] = Some(req);
        self.signal_driver = true;
    }

    /// Answer the apps whose jobs are done, and fill the free slots.
    fn harvest(&mut self) {
        for i in 0..d::JOBS {
            let Some(req) = self.slot[i] else { continue };
            let j = &self.jobs.jobs[i];
            if load(&j.posted) != load(&j.done) {
                continue;
            }
            let st = load(&j.status);
            let sum = if req.label == k::READ {
                let buf = self.data + i as u64 * SECTOR as u64;
                alaya_programs::checksum((0..(SECTOR / 8) as u64).map(|w| {
                    // SAFETY: the data pages are mapped here.
                    unsafe { core::ptr::read_volatile((buf + w * 8) as *const u64) }
                }))
            } else {
                0
            };
            let _ = alaya_sys::reply(req.reply, &Msg::new(0, &[st, sum]));
            self.give_reply(req.reply);
            self.slot[i] = None;
            if self.queued > 0 {
                let next = self.queue[0];
                self.queue.copy_within(1..self.queued, 0);
                self.queued -= 1;
                self.post(i, next);
            }
        }
        let f = load(&self.jobs.finished);
        if f != self.last_finished {
            self.last_finished = f;
            self.progress_ms = self.now();
        }
        if self.sectors == 0 {
            self.sectors = load(&self.jobs.sectors);
        }
        // An update waits for the new version to run.
        if let Some(u) = self.update
            && u.started
            && load(&self.jobs.state) == run::RUNNING
            && load(&self.jobs.version) == u.image as u64 + 1
        {
            self.update = None;
            self.updates += 1;
            let _ = alaya_sys::reply(u.reply, &Msg::new(0, &[status::OK, u.image as u64 + 1]));
            self.give_reply(u.reply);
            if u.trial_ms == 0 {
                self.commit();
            }
        }
    }

    // -----------------------------------------------------------------
    // Crashes

    fn crash_driver(&mut self) {
        if let Some(p) = self.inst[self.cur].prog {
            // Wherever it is, make it jump to address 0: a real crash.
            let _ = alaya_sys::thread_stop(p.thread);
            let _ = alaya_sys::thread_write(p.thread, 0, 0, 0, 0, 0);
            let _ = alaya_sys::thread_start(p.thread);
        }
    }

    fn freeze_driver(&mut self) {
        if let Some(p) = self.inst[self.cur].prog {
            let _ = alaya_sys::thread_stop(p.thread);
        }
    }

    /// Instance `n` crashed or hung.
    fn on_crash(&mut self, n: usize, hung: bool) {
        if self.old == Some(n) {
            // The parked old version died: no way back any more.
            self.kill(n);
            self.old = None;
            return;
        }
        if n != self.cur || self.off {
            self.kill(n);
            return;
        }
        if self.old.is_some() {
            self.rollback();
            return;
        }
        let now = self.now();
        let window = self.window;
        let mut recent = [0u64; 16];
        let mut kept = 0;
        for &t in &self.crashes[..self.ncrash] {
            if now.saturating_sub(t) < window && kept < 15 {
                recent[kept] = t;
                kept += 1;
            }
        }
        recent[kept] = now;
        self.crashes = recent;
        self.ncrash = kept + 1;
        if self.ncrash as u64 > self.limit {
            self.switch_off();
            return;
        }
        let pending = self.pending();
        let image = match self.update {
            // It died while handing over: the update goes ahead, from a
            // clean start.
            Some(u) if !u.started => {
                self.update = Some(Update { started: true, ..u });
                u.image
            }
            _ => self.inst[n].image,
        };
        if hung {
            self.hangs += 1;
        }
        self.restarts += 1;
        self.resent += pending;
        if let Err(e) = self.start(n, image, 0) {
            say!("cannot restart the driver: {e:?}");
            self.switch_off();
        }
    }

    /// Too many crashes: switch the device off (decision 78).
    fn switch_off(&mut self) {
        let cur = self.cur;
        self.kill(cur);
        if let Some(o) = self.old.take() {
            self.kill(o);
        }
        self.off = true;
        say!(
            "NOTE: the disk driver crashed more than {} times in {} seconds, so the disk is switched off. \
             Programs get a clear \"device off\" answer. Choose \"try again\" to switch it back on.",
            self.limit,
            self.window / 1000
        );
        for i in 0..d::JOBS {
            if let Some(req) = self.slot[i].take() {
                let _ = alaya_sys::reply(req.reply, &Msg::new(0, &[status::DEVICE_OFF, 0]));
                self.give_reply(req.reply);
                let j = &self.jobs.jobs[i];
                store(&j.done, load(&j.posted));
            }
        }
        for q in 0..self.queued {
            let req = self.queue[q];
            let _ = alaya_sys::reply(req.reply, &Msg::new(0, &[status::DEVICE_OFF, 0]));
            self.give_reply(req.reply);
        }
        self.queued = 0;
        if let Some(u) = self.update.take() {
            let _ = alaya_sys::reply(u.reply, &Msg::new(0, &[status::DEVICE_OFF, 0]));
            self.give_reply(u.reply);
        }
    }

    fn tick(&mut self) {
        let now = self.now();
        if self.off {
            return;
        }
        // The watchdog: work waiting, and nothing finished for too long.
        let waiting = self.pending() > 0 || load(&self.jobs.state) == run::STARTING;
        if !waiting {
            self.progress_ms = now;
        } else if now.saturating_sub(self.progress_ms) > k::WATCHDOG_MS {
            say!("the driver stopped answering: restarting it");
            self.progress_ms = now;
            let cur = self.cur;
            self.on_crash(cur, true);
            return;
        }
        if let Some(o) = self.old
            && self.update.is_none()
            && now >= self.trial_end
        {
            let _ = o;
            self.commit();
        }
        if self.chaos && self.update.is_none() && self.old.is_none() && now >= self.chaos_next {
            let gap = self.chaos_min + self.rand() % (self.chaos_max - self.chaos_min);
            self.chaos_next = now + gap;
            if self.rand().is_multiple_of(4) {
                // Leave a frozen driver to the watchdog.
                self.chaos_next = now + k::WATCHDOG_MS + gap;
                self.freeze_driver();
            } else {
                self.crash_driver();
            }
        }
    }

    // -----------------------------------------------------------------
    // Live updates

    /// Start an update; returns true if the caller waits for the answer.
    fn begin_update(&mut self, r: u64, image: usize, trial_ms: u64, flags: u64) -> bool {
        if self.off || self.update.is_some() || image > 1 {
            let _ = alaya_sys::reply(r, &Msg::new(0, &[status::BUSY, 0]));
            return false;
        }
        if self.old.is_some() {
            // A trial still runs: keep that version first.
            self.commit();
        }
        self.update = Some(Update {
            reply: r,
            image,
            trial_ms: if trial_ms == u64::MAX {
                k::TRIAL_MS
            } else {
                trial_ms
            },
            flags,
            started: false,
        });
        self.progress_ms = self.now();
        let _ = alaya_sys::signal(self.dntfn, d::PAUSE_BIT);
        true
    }

    /// The running version parked itself: start the new one.
    fn continue_update(&mut self) {
        let Some(u) = self.update else { return };
        if u.started || load(&self.jobs.state) != run::PARKED {
            return;
        }
        let new = 1 - self.cur;
        let flags = d::FLAG_HANDOVER | u.flags;
        self.update = Some(Update { started: true, ..u });
        match self.start(new, u.image, flags) {
            Ok(()) => {
                if self.updates == 0 {
                    say!(
                        "version {} takes the disk over from version {} while it runs (no reset)",
                        u.image + 1,
                        self.version_of(self.cur)
                    );
                }
                self.old = Some(self.cur);
                self.cur = new;
                self.trial_end = self.now() + u.trial_ms;
            }
            Err(e) => {
                say!("cannot start the new version: {e:?}; keeping the old one");
                self.kill(new);
                self.update = None;
                let _ = alaya_sys::reply(u.reply, &Msg::new(0, &[status::IO_ERROR, 0]));
                self.give_reply(u.reply);
                let _ = alaya_sys::signal(self.park, d::ROLLBACK_BIT);
            }
        }
    }

    /// The trial passed: the old version goes.
    fn commit(&mut self) {
        if let Some(o) = self.old.take() {
            self.kill(o);
        }
    }

    /// The new version failed during its trial: the old one comes back.
    fn rollback(&mut self) {
        let Some(o) = self.old.take() else { return };
        let bad = self.cur;
        self.kill(bad);
        self.cur = o;
        self.rollbacks += 1;
        self.resent += self.pending();
        say!(
            "NOTE: version {} failed during its trial, so version {} is back. Nothing was lost.",
            self.version_of(bad),
            self.version_of(o)
        );
        if let Some(u) = self.update.take() {
            let _ = alaya_sys::reply(
                u.reply,
                &Msg::new(0, &[status::IO_ERROR, self.version_of(o)]),
            );
            self.give_reply(u.reply);
        }
        store(&self.jobs.state, run::STARTING);
        self.progress_ms = self.now();
        let _ = alaya_sys::signal(self.park, d::ROLLBACK_BIT);
    }
}
