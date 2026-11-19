import { useCallback, useEffect, useRef, useState } from "react";
import { GITHUB_URL } from "./seo";

const LINES = ["who?", "who: .customer.name", "no joins.", "I read your schema.", "ask me at night.", "you again?"];

/** How far a pupil moves toward a point, capped at `reach`. */
function pull(box: DOMRect, x: number, y: number, reach: number) {
  const dx = x - (box.left + box.width / 2);
  const dy = y - (box.top + box.height / 2);
  const far = Math.hypot(dx, dy) || 1;
  const by = Math.min(1, far / 220) * reach;
  return [(dx / far) * by, (dy / far) * by];
}

/**
 * Everything that moves on the page after it has loaded: eyes that follow the cursor, the owl's
 * speech and hop, the sentence that fills in on scroll, sections that rise into view, and the
 * keyboard shortcuts. The page is complete without any of it.
 */
export function usePlay() {
  const [line, setLine] = useState<string | null>(null);
  const hush = useRef<ReturnType<typeof setTimeout>>(undefined);
  const next = useRef(0);

  const say = useCallback((text: string, ms = 2200) => {
    setLine(text);
    clearTimeout(hush.current);
    hush.current = setTimeout(() => setLine(null), ms);
  }, []);

  const hoot = useCallback(() => {
    say(LINES[next.current++ % LINES.length]);
    const owl = document.getElementById("owl");
    if (!owl || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    owl.classList.remove("hop");
    void owl.getBoundingClientRect(); // restart the animation
    owl.classList.add("hop");
  }, [say]);

  useEffect(() => {
    const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const owl = document.getElementById("owl")!;
    const undo: Array<() => void> = [];
    const on = <K extends keyof WindowEventMap>(type: K, fn: (e: WindowEventMap[K]) => void) => {
      addEventListener(type, fn, { passive: true });
      undo.push(() => removeEventListener(type, fn));
    };

    // Eyes, blinking, and noticing the mouse.
    if (!calm) {
      const owlPupils = [...owl.querySelectorAll<SVGCircleElement>(".pupil")];
      const otherEyes = [...document.querySelectorAll<HTMLElement>(".look")];
      let near = false;
      on("pointermove", (e) => {
        for (const pupil of owlPupils) {
          const [dx, dy] = pull(pupil.previousElementSibling!.getBoundingClientRect(), e.clientX, e.clientY, 13);
          pupil.setAttribute("transform", `translate(${dx} ${dy})`);
        }
        for (const eye of otherEyes) {
          const [dx, dy] = pull(eye.getBoundingClientRect(), e.clientX, e.clientY, Number(eye.dataset.reach));
          (eye.firstElementChild as HTMLElement).style.transform = `translate(${dx}px, ${dy}px)`;
        }
        if (e.pointerType !== "mouse") return;
        const box = owl.getBoundingClientRect();
        const far = Math.hypot(e.clientX - (box.left + box.width / 2), e.clientY - (box.top + box.height / 2));
        if (!near && far < 150) {
          near = true;
          say("a mouse!");
        } else if (near && far > 280) near = false;
      });

      let blinkTimer: ReturnType<typeof setTimeout>;
      const blink = () => {
        owl.classList.add("blink");
        setTimeout(() => owl.classList.remove("blink"), 130);
        blinkTimer = setTimeout(blink, 2600 + Math.random() * 3800);
      };
      blink();
      const hello = setTimeout(() => scrollY < 200 && say("who?"), 1800);
      undo.push(() => (clearTimeout(blinkTimer), clearTimeout(hello)));
    }

    // The sentence fills in word by word as it crosses the screen.
    const sentence = document.getElementById("say")!;
    const words = [...sentence.children];
    const fill = () => {
      const box = sentence.getBoundingClientRect();
      const start = innerHeight * 0.85;
      const end = innerHeight * 0.3;
      const done = (start - box.top) / (start - end + box.height * 0.6);
      const upTo = calm ? words.length : Math.round(Math.max(0, Math.min(1, done)) * words.length);
      words.forEach((w, i) => w.classList.toggle("on", i < upTo));
    };
    on("scroll", fill);
    fill();

    // Sections rise, and the evidence tiles pop, when they come into view.
    const seen = new IntersectionObserver(
      (entries) =>
        entries.forEach((e) => {
          if (!e.isIntersecting) return;
          e.target.classList.add("in");
          seen.unobserve(e.target);
        }),
      { rootMargin: "0px 0px -8% 0px" },
    );
    document.querySelectorAll(".band .wrap > *, .night .wrap > *, .card").forEach((el) => {
      el.classList.add("rise");
      seen.observe(el);
    });
    document.querySelectorAll(".ticks").forEach((el) => seen.observe(el));
    undo.push(() => seen.disconnect());

    // The keys shown on the page.
    const keys = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || /input|textarea/i.test((e.target as HTMLElement).tagName)) return;
      const k = e.key.toLowerCase();
      if (k === "l") document.getElementById("language")?.scrollIntoView();
      if (k === "g") window.open(GITHUB_URL, "_blank", "noopener,noreferrer");
      if (k === "h") {
        scrollTo({ top: 0 });
        hoot();
      }
    };
    addEventListener("keydown", keys);
    undo.push(() => removeEventListener("keydown", keys));

    return () => undo.forEach((fn) => fn());
  }, [say, hoot]);

  return { line, say, hoot };
}
