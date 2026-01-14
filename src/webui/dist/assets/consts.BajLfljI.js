import { c as createComponent, d as createAstro, e as addAttribute, b as renderScript, a as renderTemplate, f as renderHead, r as renderComponent, g as renderSlot } from './astro/server.BnM-xRux.js';
import 'piccolore';
import 'html-escaper';
/* empty css                               */
import 'clsx';

const $$Astro$1 = createAstro();
const $$ClientRouter = createComponent(($$result, $$props, $$slots) => {
  const Astro2 = $$result.createAstro($$Astro$1, $$props, $$slots);
  Astro2.self = $$ClientRouter;
  const { fallback = "animate" } = Astro2.props;
  return renderTemplate`<meta name="astro-view-transitions-enabled" content="true"><meta name="astro-view-transitions-fallback"${addAttribute(fallback, "content")}>${renderScript($$result, "/home/runner/work/opm/opm/src/webui/node_modules/astro/components/ClientRouter.astro?astro&type=script&index=0&lang.ts")}`;
}, "/home/runner/work/opm/opm/src/webui/node_modules/astro/components/ClientRouter.astro", void 0);

const $$Astro = createAstro();
const $$Base = createComponent(($$result, $$props, $$slots) => {
  const Astro2 = $$result.createAstro($$Astro, $$props, $$slots);
  Astro2.self = $$Base;
  const { title, description } = Astro2.props;
  return renderTemplate`<html lang="en"> <head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="generator"${addAttribute(Astro2.generator, "content")}><!-- Cache Control Meta Tags --><meta http-equiv="Cache-Control" content="no-cache, no-store, must-revalidate"><meta http-equiv="Pragma" content="no-cache"><meta http-equiv="Expires" content="0"><title>${title}</title><link rel="icon" type="image/svg+xml" href="{{base_path | safe}}/assets/favicon.svg"><link rel="stylesheet" href="https://rsms.me/inter/inter.css">${renderHead()}</head>${renderComponent($$result, "app-redirect", "app-redirect", { "data-base": "{{base_path | safe}}" })} <meta name="title"${addAttribute(title, "content")}> <meta name="description"${addAttribute(description, "content")}> <meta property="og:type" content="website"> <meta property="og:title"${addAttribute(title, "content")}> <meta property="og:description"${addAttribute(description, "content")}> <meta property="og:image" content="{{base_path | safe}}/assets/banner.png"> <meta property="twitter:card" content="summary_large_image"> <meta property="twitter:title"${addAttribute(title, "content")}> <meta property="twitter:description"${addAttribute(description, "content")}> <meta property="twitter:image" content="{{base_path | safe}}/assets/banner.png"> ${renderComponent($$result, "ViewTransitions", $$ClientRouter, {})} ${renderScript($$result, "/home/runner/work/opm/opm/src/webui/src/components/base.astro?astro&type=script&index=0&lang.ts")} <body> ${renderSlot($$result, $$slots["default"])} </body></html>`;
}, "/home/runner/work/opm/opm/src/webui/src/components/base.astro", void 0);

const SITE_TITLE = "OPM";
const SITE_DESCRIPTION = "Open Process Management";

export { $$Base as $, SITE_DESCRIPTION as S, SITE_TITLE as a };
