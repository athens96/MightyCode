// The home page's motion: the hero shot settling flat, the statement's words lighting up, the two sticky diagrams that grow with
// the scroll, the phone's notification → approval sheet → approved, and the reveal-on-view tiles and counters. app.js owns the
// language; its words come from window.mcT and redraw on its "mc-lang" event. With reduced motion none of this runs and home.css
// shows each section finished.
(function () {
  const body = document.body;
  // with reduced motion the diagrams are drawn once, finished, and redrawn only when the language changes
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!still) body.classList.add("motion");

  const t = (key) => (window.mcT ? window.mcT(key) : "");
  const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));

  // the agent ribbon loops seamlessly over two copies; the copy is hidden from screen readers
  const track = document.getElementById("track");
  if (track && !still) {
    track.querySelectorAll(".chip").forEach((chip) => {
      const copy = chip.cloneNode(true);
      copy.setAttribute("aria-hidden", "true");
      copy.tabIndex = -1;
      track.appendChild(copy);
    });
  }

  // the statement, split into words (characters for Chinese and Japanese, which have no spaces) that light up with the scroll
  const statement = document.getElementById("words");
  let words = [];
  function splitStatement() {
    if (!statement) return;
    const html = t("home.statement") || statement.innerHTML;
    const lang = window.mcLang ? window.mcLang() : "ko";
    const byChar = lang === "zh" || lang === "ja";
    statement.setAttribute("aria-label", html.replace(/<br>/g, " "));
    statement.innerHTML = html
      .split(/(<br>)/)
      .map((part) => {
        if (part === "<br>") return part;
        const pieces = byChar ? Array.from(part) : part.split(/(\s+)/);
        return pieces.map((p) => (/^\s+$/.test(p) ? p : `<span class="w" aria-hidden="true">${p}</span>`)).join("");
      })
      .join("");
    words = [...statement.querySelectorAll(".w")];
  }

  // reveal on view, and count numbers up the first time they show
  const io = new IntersectionObserver(
    (entries) =>
      entries.forEach((e) => {
        if (!e.isIntersecting) return;
        e.target.classList.add("in");
        io.unobserve(e.target);
        e.target.querySelectorAll("[data-count]").forEach(countUp);
      }),
    { threshold: 0.2 }
  );
  document.querySelectorAll(".reveal").forEach((el) => io.observe(el));
  function countUp(el) {
    const end = Number(el.dataset.count);
    if (still) return;
    const start = performance.now();
    const tick = (now) => {
      const k = clamp((now - start) / 1100);
      el.textContent = String(Math.round(end * (1 - Math.pow(1 - k, 3))));
      if (k < 1) requestAnimationFrame(tick);
    };
    el.textContent = "0";
    requestAnimationFrame(tick);
  }

  // the orchestration links draw on through their own length
  document.querySelectorAll(".link").forEach((p) => p.style.setProperty("--len", String(Math.ceil(p.getTotalLength()))));

  // the sticky sections stand below the top bar, which wraps to more rows on a narrow screen
  const topbar = document.querySelector(".topbar");
  const measureTop = () => topbar && document.documentElement.style.setProperty("--h-top", topbar.offsetHeight + "px");
  measureTop();
  addEventListener("resize", measureTop);

  const hero = document.getElementById("heroShot");
  const phone = document.getElementById("phoneMock");
  const scrollies = [...document.querySelectorAll("[data-scrolly]")];

  function progress(el) {
    const r = el.getBoundingClientRect();
    return clamp(-r.top / Math.max(1, r.height - innerHeight));
  }

  function setState(block, done, time) {
    block.classList.toggle("done", done);
    block.classList.toggle("run", !done && !block.classList.contains("res"));
    const m = block.querySelector("[data-m]");
    const word = done ? `${t("home.st.done")} · ${time}` : block.classList.contains("res") ? t("home.st.writing") : t("home.st.running");
    if (m.textContent !== word) m.textContent = word;
  }

  function flow(p, root) {
    const stage = p < 0.16 ? 0 : p < 0.36 ? 1 : p < 0.54 ? 2 : p < 0.7 ? 3 : 4;
    root.querySelectorAll("[data-k]").forEach((el) => el.classList.toggle("in", Number(el.dataset.k) <= stage));
    const step = stage < 1 ? 0 : stage < 3 ? 1 : 2;
    root.querySelectorAll(".step").forEach((s) => s.classList.toggle("on", Number(s.dataset.at) === step));
    const [s1, s2, res] = ["fs1", "fs2", "fres"].map((id) => root.querySelector("#" + id));
    s1.querySelector(".pbar > i").style.width = clamp((p - 0.16) / 0.2) * 100 + "%";
    s2.querySelector(".pbar > i").style.width = clamp((p - 0.36) / 0.3) * 100 + "%";
    setState(s1, p > 0.38, "0:08");
    setState(s2, p > 0.66, "1:12");
    const arrived = p > 0.78;
    setState(res, arrived, "4:21");
    // the view glides to the result once it arrives, as the app's diagram does
    root.querySelector(".cam").style.transform = arrived && !still ? `translateY(${-root.querySelector(".canvas").clientHeight * 0.26}px) scale(1.1)` : "none";
  }

  function orch(p, root) {
    const stage = p < 0.12 ? 0 : p < 0.45 ? 1 : p < 0.72 ? 2 : 3;
    root.querySelectorAll("[data-k]").forEach((el) => el.classList.toggle("in", Number(el.dataset.k) <= stage));
    const step = stage <= 1 ? (stage === 0 ? 0 : 1) : stage === 2 ? 1 : 2;
    root.querySelectorAll(".step").forEach((s) => s.classList.toggle("on", Number(s.dataset.at) === step));
    root.querySelectorAll(".child").forEach((c, i) => {
      const k = clamp((p - 0.2 - i * 0.06) / 0.4);
      c.querySelector(".prog").setAttribute("width", String(128 * k));
      c.classList.toggle("ok", k >= 1);
    });
  }

  function frame() {
    if (hero) {
      // the tilted shot settles flat; once flat the transform is dropped so the browser draws it at full sharpness
      const k = clamp(scrollY / (innerHeight * 0.6));
      hero.style.transform = k >= 1 ? "none" : `rotateX(${(1 - k) * 18}deg) translateY(${(1 - k) * 24}px)`;
    }
    if (statement && words.length) {
      const r = statement.getBoundingClientRect();
      const k = clamp((innerHeight * 0.85 - r.top) / (innerHeight * 0.6));
      words.forEach((w, i) => w.classList.toggle("lit", i / words.length < k));
    }
    scrollies.forEach((s) => (s.dataset.scrolly === "flow" ? flow : orch)(progress(s), s));
    if (phone) {
      const r = phone.getBoundingClientRect();
      const k = clamp((innerHeight - r.top) / (innerHeight + r.height * 0.3));
      phone.classList.toggle("s1", k > 0.35 && k <= 0.6);
      phone.classList.toggle("s2", k > 0.6);
      phone.classList.toggle("s3", k > 0.85);
    }
  }

  if (still) {
    const finished = () => scrollies.forEach((s) => (s.dataset.scrolly === "flow" ? flow : orch)(1, s));
    document.addEventListener("mc-lang", finished);
    finished();
    return;
  }

  let queued = false;
  function schedule() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      frame();
    });
  }
  addEventListener("scroll", schedule, { passive: true });
  addEventListener("resize", schedule);
  document.addEventListener("mc-lang", () => {
    splitStatement();
    frame();
  });
  splitStatement();
  frame();
})();
