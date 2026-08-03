import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import { useRoute } from "vitepress";
import { h, onMounted, onUnmounted, watch, nextTick } from "vue";
import { initBanner } from "./banner";
import EndevFooter from "./EndevFooter.vue";
import EndevSponsors from "./EndevSponsors.vue";
import { data as starsData } from "../stars.data";
import "./custom.css";

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      "layout-bottom": () => [h(EndevSponsors), h(EndevFooter)],
    });
  },
  enhanceApp() {
    initBanner();
  },
  setup() {
    const route = useRoute();
    let mermaidObserver: MutationObserver | undefined;

    // Mermaid 11.15+ #7759 lowercases `foreignObject` in themeCSS, making CSS
    // rules miss. Mermaid renders asynchronously, so observe later node inserts.
    const fixMermaidClipping = () => {
      document
        .querySelectorAll<SVGForeignObjectElement>(".mermaid foreignObject")
        .forEach((fo) => {
          fo.setAttribute("overflow", "visible");
          const div = fo.querySelector("div");
          if (div) div.style.overflow = "visible";
        });
    };
    onMounted(() => {
      fixMermaidClipping();
      mermaidObserver = new MutationObserver(fixMermaidClipping);
      mermaidObserver.observe(document.body, {
        childList: true,
        subtree: true,
      });
    });
    watch(() => route.path, () => nextTick(fixMermaidClipping));

    let observer: MutationObserver | undefined;
    onMounted(() => {
      const addStarCount = () => {
        if (!starsData.stars) return false;

        const githubLinks = document.querySelectorAll(
          '.VPSocialLinks a[href*="github.com/jdx/pitchfork"]',
        );
        githubLinks.forEach((githubLink) => {
          if (!githubLink.querySelector(".star-count")) {
            const starBadge = document.createElement("span");
            starBadge.className = "star-count";
            starBadge.title = "GitHub Stars";
            const glyph = document.createElement("span");
            glyph.className = "star-glyph";
            glyph.textContent = "★";
            glyph.setAttribute("aria-hidden", "true");
            starBadge.append(glyph, starsData.stars);
            githubLink.appendChild(starBadge);
          }
        });
        return (
          githubLinks.length > 0 &&
          Array.from(githubLinks).every((link) =>
            link.querySelector(".star-count"),
          )
        );
      };

      if (addStarCount()) return;

      observer = new MutationObserver(() => {
        if (addStarCount()) observer?.disconnect();
      });
      observer.observe(document.querySelector(".VPNav") || document.body, {
        childList: true,
        subtree: true,
      });
    });
    onUnmounted(() => {
      observer?.disconnect();
      mermaidObserver?.disconnect();
    });
  },
} satisfies Theme;
