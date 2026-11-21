import { useCallback, useEffect, useRef, useState } from "react";
import { pull } from "./usePlay";

/**
 * The owl on a page other than the home page: its eyes follow the cursor, it blinks, and it can
 * be told to say something or to hop. The page is complete without any of it.
 */
export function usePerch() {
  const [line, setLine] = useState<string | null>(null);
  const hush = useRef<ReturnType<typeof setTimeout>>(undefined);

  const say = useCallback((text: string, ms = 2400) => {
    setLine(text);
    clearTimeout(hush.current);
    hush.current = setTimeout(() => setLine(null), ms);
  }, []);

  const hop = useCallback(() => {
    const owl = document.getElementById("owl");
    if (!owl || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    owl.classList.remove("hop");
    void owl.getBoundingClientRect(); // restart the animation
    owl.classList.add("hop");
  }, []);

  useEffect(() => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const owl = document.getElementById("owl");
    if (!owl) return;
    const pupils = [...owl.querySelectorAll<SVGCircleElement>(".pupil")];
    const others = [...document.querySelectorAll<HTMLElement>(".look")];
    const follow = (e: PointerEvent) => {
      for (const pupil of pupils) {
        const [dx, dy] = pull(pupil.previousElementSibling!.getBoundingClientRect(), e.clientX, e.clientY, 13);
        pupil.setAttribute("transform", `translate(${dx} ${dy})`);
      }
      for (const eye of others) {
        const [dx, dy] = pull(eye.getBoundingClientRect(), e.clientX, e.clientY, Number(eye.dataset.reach));
        (eye.firstElementChild as HTMLElement).style.transform = `translate(${dx}px, ${dy}px)`;
      }
    };
    addEventListener("pointermove", follow, { passive: true });
    let timer: ReturnType<typeof setTimeout>;
    const blink = () => {
      owl.classList.add("blink");
      setTimeout(() => owl.classList.remove("blink"), 130);
      timer = setTimeout(blink, 2600 + Math.random() * 3800);
    };
    blink();
    return () => {
      removeEventListener("pointermove", follow);
      clearTimeout(timer);
      clearTimeout(hush.current);
    };
  }, []);

  return { line, say, hop };
}
