// 空状态插图：缩小版的 TailTunnel 界面。

export type EmptyScene = "no-tunnels" | "lonely-tailnet" | "no-tailscale";

const ICONS = {
  add: "M11 11V5H13V11H19V13H13V19H11V13H5V11H11Z",
  refresh:
    "M5.46257 4.43262C7.21556 2.91688 9.5007 2 12 2C17.5228 2 22 6.47715 22 12C22 14.1361 21.3302 16.1158 20.1892 17.7406L17 12H20C20 7.58172 16.4183 4 12 4C9.84982 4 7.89777 4.84827 6.46023 6.22842L5.46257 4.43262ZM18.5374 19.5674C16.7844 21.0831 14.4993 22 12 22C6.47715 22 2 17.5228 2 12C2 9.86386 2.66979 7.88416 3.8108 6.25944L7 12H4C4 16.4183 7.58172 20 12 20C14.1502 20 16.1022 19.1517 17.5398 17.7716L18.5374 19.5674Z",
  route:
    "M4 15V8.5C4 6.01472 6.01472 4 8.5 4C10.9853 4 13 6.01472 13 8.5V15.5C13 16.8807 14.1193 18 15.5 18C16.8807 18 18 16.8807 18 15.5V8.82929C16.8348 8.41746 16 7.30622 16 6C16 4.34315 17.3431 3 19 3C20.6569 3 22 4.34315 22 6C22 7.30622 21.1652 8.41746 20 8.82929V15.5C20 17.9853 17.9853 20 15.5 20C13.0147 20 11 17.9853 11 15.5V8.5C11 7.11929 9.88071 6 8.5 6C7.11929 6 6 7.11929 6 8.5V15H9L5 20L1 15H4ZM19 7C19.5523 7 20 6.55228 20 6C20 5.44772 19.5523 5 19 5C18.4477 5 18 5.44772 18 6C18 6.55228 18.4477 7 19 7Z",
  macbook: "M4 5V16H20V5H4ZM2 4.00748C2 3.45107 2.45531 3 2.9918 3H21.0082C21.556 3 22 3.44892 22 4.00748V18H2V4.00748ZM1 19H23V21H1V19Z",
  wifiOff:
    "M12.0001 18C12.7144 18 13.3704 18.2497 13.8856 18.6665L12.0001 21L10.1145 18.6665C10.6297 18.2497 11.2857 18 12.0001 18ZM2.80766 1.39343L20.4853 19.0711L19.0711 20.4853L13.8913 15.3042C13.2967 15.1069 12.6609 15 12.0001 15C10.5719 15 9.26024 15.499 8.22998 16.3322L6.97363 14.7759C8.24961 13.7442 9.84925 13.0969 11.5964 13.01L9.00025 10.414C7.55273 10.8234 6.22651 11.5217 5.0878 12.4426L3.83099 10.8868C4.89946 10.0226 6.10763 9.32438 7.41633 8.83118L5.13168 6.5451C3.98878 7.08913 2.92058 7.76472 1.94666 8.55228L0.689453 6.99674C1.60358 6.25747 2.59156 5.60589 3.64058 5.05479L1.39345 2.80765L2.80766 1.39343ZM14.5004 10.2854L12.2165 8.00243L12 8C15.0947 8 17.9369 9.08141 20.1693 10.8869L18.9123 12.4426C17.6438 11.4167 16.1427 10.6672 14.5004 10.2854ZM12.0001 3.00003C16.2849 3.00003 20.22 4.49719 23.3109 6.99691L22.0534 8.55228C19.3061 6.33062 15.8085 5.00003 12.0001 5.00003C11.122 5.00003 10.2604 5.07077 9.42075 5.20685L7.72455 3.51088C9.09498 3.17702 10.5268 3.00003 12.0001 3.00003Z",
  power:
    "M6.26489 3.80698L7.41191 5.44558C5.34875 6.89247 4 9.28873 4 12C4 16.4183 7.58172 20 12 20C16.4183 20 20 16.4183 20 12C20 9.28873 18.6512 6.89247 16.5881 5.44558L17.7351 3.80698C20.3141 5.61559 22 8.61091 22 12C22 17.5228 17.5228 22 12 22C6.47715 22 2 17.5228 2 12C2 8.61091 3.68594 5.61559 6.26489 3.80698ZM11 12V2H13V12H11Z",
};

function icon(d: string, cx: number, cy: number, size: number, cls: string): string {
  const s = size / 24;
  return `<path class="${cls}" transform="translate(${cx - size / 2} ${cy - size / 2}) scale(${s})" d="${d}"/>`;
}

function bar(x: number, y: number, w: number, strong = false): string {
  const h = strong ? 5 : 4;
  return `<rect class="${strong ? "bar-strong" : "bar"}" x="${x}" y="${y}" width="${w}" height="${h}" rx="${h / 2}"/>`;
}

function style(p: string): string {
  const light = `--base:#fafafa;--hatch:rgba(15,23,42,0.05);--edge:rgba(15,23,42,0.07);--surface:#ffffff;--stroke:rgba(47,42,34,0.08);--tab:#eef1f5;--bar:#e5e9ef;--bar-strong:#cdd4de;--cell:#f1f3f6;--back:#f3f3f4;--icon:#a3abb7;--icon-2:#6b7280;--accent:#3462ad;--shadow:rgb(47,42,34);--shadow-o:0.13;`;
  const dark = `--base:#26262b;--hatch:rgba(255,255,255,0.035);--edge:rgba(255,255,255,0.06);--surface:#323238;--stroke:rgba(255,255,255,0.06);--tab:#2a2a2f;--bar:#3f3f47;--bar-strong:#52525b;--cell:#2a2a2f;--back:#2c2c31;--icon:#7d828c;--icon-2:#9a9fa8;--accent:#7ea6ea;--shadow:#000000;--shadow-o:0.36;`;
  return `<style>
#${p}{${light}}
#${p}[data-theme="dark"]{${dark}}
@media (prefers-color-scheme: dark){#${p}[data-theme="auto"]{${dark}}}
#${p} .base{fill:var(--base)}
#${p} .hatch-line{stroke:var(--hatch)}
#${p} .edge{fill:none;stroke:var(--edge)}
#${p} .surface{fill:var(--surface);stroke:var(--stroke)}
#${p} .back{fill:var(--back);stroke:var(--stroke)}
#${p} .tab{fill:var(--tab)}
#${p} .rule{stroke:var(--stroke)}
#${p} .bar{fill:var(--bar)}
#${p} .bar-strong{fill:var(--bar-strong)}
#${p} .cell{fill:var(--cell)}
#${p} .icon{fill:var(--icon)}
#${p} .icon-2{fill:var(--icon-2)}
#${p} .accent{fill:var(--accent)}
#${p} .on-key{fill:#ffffff}
#${p} .flood{flood-color:var(--shadow);flood-opacity:var(--shadow-o)}
@media (prefers-reduced-motion: no-preference){
#${p} .pop{transform-box:fill-box;transform-origin:center;animation:${p}-pop 520ms cubic-bezier(0.34,1.56,0.64,1) 180ms both}
}
@keyframes ${p}-pop{from{transform:scale(0.2)}to{transform:scale(1)}}
</style>`;
}

function defs(p: string): string {
  return `<defs>
<pattern id="${p}-hatch" width="7" height="7" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line class="hatch-line" x1="0" y1="0" x2="0" y2="7" stroke-width="1"/></pattern>
<filter id="${p}-shadow" x="-30%" y="-30%" width="160%" height="170%" color-interpolation-filters="sRGB">
<feGaussianBlur in="SourceAlpha" stdDeviation="4.5" result="blur"/>
<feOffset in="blur" dy="5" result="drop"/>
<feFlood class="flood" result="tint"/>
<feComposite in="tint" in2="drop" operator="in" result="shade"/>
<feMerge><feMergeNode in="shade"/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
<linearGradient id="${p}-slate" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#404859"/><stop offset="1" stop-color="#273042"/></linearGradient>
</defs>`;
}

function board(p: string): string {
  return `<rect class="base" width="160" height="120" rx="18"/>
<rect width="160" height="120" rx="18" fill="url(#${p}-hatch)"/>
<rect class="edge" x="0.5" y="0.5" width="159" height="119" rx="17.5"/>`;
}

// 还没有隧道：后面一张卡片，前面一个 TailTunnel 窗口，右下角是新建按钮。
function noTunnels(p: string): string {
  return `<g filter="url(#${p}-shadow)">
<rect class="back" x="36.5" y="18.5" width="100" height="60" rx="11"/>
</g>
<g filter="url(#${p}-shadow)">
<rect class="surface" x="22.5" y="28.5" width="104" height="70" rx="11"/>
</g>
<path class="tab" d="M23 39.5a10.5 10.5 0 0 1 10.5-10.5h82a10.5 10.5 0 0 1 10.5 10.5V46H23z"/>
<line class="rule" x1="23" y1="46.5" x2="126" y2="46.5"/>
<circle class="bar-strong" cx="31" cy="37.5" r="1.8"/>
<circle class="bar-strong" cx="36.5" cy="37.5" r="1.8"/>
<circle class="bar-strong" cx="42" cy="37.5" r="1.8"/>
${bar(64, 35.5, 22)}
<line class="rule" x1="53.5" y1="47" x2="53.5" y2="98"/>
<rect class="cell" x="26" y="51" width="24" height="10" rx="4"/>
${bar(29, 54, 15)}
${bar(29, 66, 12)}
${bar(29, 76, 16)}
${bar(59, 52, 26, true)}
<rect class="cell" x="59" y="62" width="61" height="30" rx="6"/>
${icon(ICONS.route, 89.5, 73, 12, "icon")}
${bar(80, 83, 19)}
<g filter="url(#${p}-shadow)">
<g class="pop">
<circle class="accent" cx="125" cy="92" r="10"/>
${icon(ICONS.add, 125, 92, 12, "on-key")}
</g>
</g>`;
}

// Tailscale 网里只有本机：机器列表第一行是本机，后面两行空着，右下角是刷新按钮。
function lonelyTailnet(p: string): string {
  return `<g filter="url(#${p}-shadow)">
<rect class="back" x="33.5" y="18.5" width="96" height="58" rx="11"/>
</g>
<g filter="url(#${p}-shadow)">
<rect class="surface" x="22.5" y="27.5" width="112" height="70" rx="12"/>
</g>
<rect class="cell" x="30" y="34" width="16" height="16" rx="5"/>
${icon(ICONS.macbook, 38, 42, 10, "icon-2")}
${bar(51, 36.5, 30, true)}
${bar(51, 44.5, 20)}
<rect class="cell" x="107" y="38.5" width="20" height="7" rx="3.5"/>
<line class="rule" x1="30" y1="56.5" x2="127" y2="56.5"/>
<rect class="cell" x="30" y="62" width="97" height="12" rx="5"/>
<rect class="cell" x="30" y="78" width="97" height="12" rx="5"/>
<g filter="url(#${p}-shadow)">
<g class="pop">
<circle class="accent" cx="127" cy="92" r="9.5"/>
${icon(ICONS.refresh, 127, 92, 11, "on-key")}
</g>
</g>`;
}

// 读不到 Tailscale：状态卡片中间是连不上的网络图标，右下角是石板灰的电源按键。没有下一步，所以没有强调色。
function noTailscale(p: string): string {
  return `<g filter="url(#${p}-shadow)">
<rect class="back" x="35.5" y="18.5" width="90" height="58" rx="11"/>
</g>
<g filter="url(#${p}-shadow)">
<rect class="surface" x="25.5" y="27.5" width="108" height="68" rx="12"/>
</g>
<circle class="cell" cx="79.5" cy="50" r="12.5"/>
${icon(ICONS.wifiOff, 79.5, 50, 13, "icon")}
${bar(60, 70, 39, true)}
${bar(66, 79, 27)}
<g filter="url(#${p}-shadow)">
<rect x="114" y="80" width="22" height="20" rx="6" fill="url(#${p}-slate)"/>
${icon(ICONS.power, 125, 90, 11, "on-key")}
</g>`;
}

export function emptyArt(scene: EmptyScene, theme: "light" | "dark" | "auto" = "auto"): string {
  const p = `tt${Math.random().toString(36).slice(2, 9)}`;
  const body = scene === "no-tunnels"
    ? noTunnels(p)
    : scene === "lonely-tailnet"
      ? lonelyTailnet(p)
      : noTailscale(p);
  return `<svg id="${p}" data-theme="${theme}" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 120" width="160" height="120" aria-hidden="true">${style(p)}${defs(p)}${board(p)}${body}</svg>`;
}
