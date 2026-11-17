// 外观：跟随系统 / 浅色 / 深色。
// 选择存在 localStorage；<html data-theme> 永远是解析后的 light 或 dark，样式只认它。
// index.html 里有一段同样逻辑的内联脚本，让首帧就是对的颜色。

export type ThemeChoice = "system" | "light" | "dark";

const KEY = "tailtunnel-theme";
const dark = window.matchMedia("(prefers-color-scheme: dark)");

export function storedChoice(): ThemeChoice {
  try {
    const value = localStorage.getItem(KEY);
    if (value === "light" || value === "dark") return value;
  } catch {
    // 读不到存储时按跟随系统处理。
  }
  return "system";
}

function resolve(choice: ThemeChoice): "light" | "dark" {
  if (choice === "system") return dark.matches ? "dark" : "light";
  return choice;
}

function apply(choice: ThemeChoice) {
  document.documentElement.dataset.theme = resolve(choice);
}

function paint(choice: ThemeChoice, buttons: NodeListOf<HTMLButtonElement>) {
  for (const button of buttons) {
    const on = button.dataset.themeChoice === choice;
    button.classList.toggle("current", on);
    button.setAttribute("aria-pressed", String(on));
  }
}

export function initTheme() {
  let choice = storedChoice();
  apply(choice);
  const buttons = document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]");
  paint(choice, buttons);
  for (const button of buttons) {
    button.addEventListener("click", () => {
      choice = button.dataset.themeChoice as ThemeChoice;
      try {
        if (choice === "system") localStorage.removeItem(KEY);
        else localStorage.setItem(KEY, choice);
      } catch {
        // 存不下来也让本次切换生效。
      }
      apply(choice);
      paint(choice, buttons);
    });
  }
  // 跟随系统时，系统切换深浅色要立刻跟上。
  dark.addEventListener("change", () => {
    if (choice === "system") apply(choice);
  });
}
