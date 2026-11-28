(() => {
  "use strict";

  const offset = String(-new Date().getTimezoneOffset() * 60);
  const name = Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  const secure = location.protocol === "https:" ? "; Secure" : "";
  for (const [key, value] of [["opengateway_tz", offset], ["opengateway_tzname", encodeURIComponent(name)]]) {
    document.cookie = `${key}=${value}; Path=/admin; SameSite=Strict; Max-Age=31536000${secure}`;
  }

  const current = document.querySelector(".bar nav [aria-current]");
  if (current) {
    current.scrollIntoView({ block: "nearest", inline: "center" });
  }

  const ticked = new WeakMap();
  const selecting = (element) => {
    const selection = getSelection();
    return selection && !selection.isCollapsed && element.contains(selection.anchorNode);
  };
  const tick = (force) => {
    if (document.hidden || typeof htmx === "undefined") {
      return;
    }
    const now = Date.now();
    for (const element of document.querySelectorAll("[data-live]")) {
      const every = Number(element.dataset.live) * 1000;
      if (!ticked.has(element)) {
        ticked.set(element, now);
        continue;
      }
      if ((force || now - ticked.get(element) >= every) && !selecting(element)) {
        ticked.set(element, now);
        htmx.trigger(element, "gwtick");
      }
    }
  };
  setInterval(() => tick(false), 1000);
  document.addEventListener("visibilitychange", () => tick(true));

  const hideTips = (except) => {
    for (const figure of document.querySelectorAll("figure.chart.hovering")) {
      if (figure === except) {
        continue;
      }
      figure.classList.remove("hovering");
      const tip = figure.querySelector(".tip");
      if (tip) {
        tip.hidden = true;
      }
      const line = figure.querySelector(".crosshair");
      if (line) {
        line.setAttribute("visibility", "hidden");
      }
    }
  };

  const show = (hit, event) => {
    const figure = hit.closest("figure.chart");
    const table = figure && figure.querySelector("details.data table");
    const tip = figure && figure.querySelector(".tip");
    if (!table || !tip) {
      return;
    }
    const row = table.tBodies[0] && table.tBodies[0].rows[Number(hit.dataset.i)];
    if (!row) {
      return;
    }
    hideTips(figure);
    figure.classList.add("hovering");
    const heads = [...table.tHead.rows[0].cells].map((cell) => cell.textContent);
    const keys = new Map(
      [...figure.querySelectorAll(".legend li")].map((item) => [
        item.textContent.trim(),
        item.querySelector("svg").getAttribute("class"),
      ]),
    );
    tip.replaceChildren();
    const head = document.createElement("div");
    head.className = "tip-head";
    head.textContent = row.cells[0].textContent;
    tip.append(head);
    for (let index = 1; index < row.cells.length; index += 1) {
      const line = document.createElement("div");
      line.className = "tip-row";
      const key = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      key.setAttribute("viewBox", "0 0 14 10");
      const keyClass = keys.get((heads[index] || "").trim());
      key.setAttribute("class", keyClass || "key");
      if (keyClass) {
        const mark = document.createElementNS("http://www.w3.org/2000/svg", "line");
        for (const [name, value] of [["x1", "0"], ["x2", "14"], ["y1", "5"], ["y2", "5"]]) {
          mark.setAttribute(name, value);
        }
        key.append(mark);
      }
      const value = document.createElement("b");
      value.textContent = row.cells[index].textContent;
      const name = document.createElement("span");
      name.textContent = heads[index] || "";
      line.append(key, value, name);
      tip.append(line);
    }
    const crosshair = figure.querySelector(".crosshair");
    if (crosshair) {
      crosshair.setAttribute("x1", hit.dataset.x);
      crosshair.setAttribute("x2", hit.dataset.x);
      crosshair.setAttribute("visibility", "visible");
    }
    tip.hidden = false;
    const box = figure.getBoundingClientRect();
    const x = event.clientX - box.left;
    const y = event.clientY - box.top;
    const width = tip.offsetWidth;
    const left = x + 16 + width > box.width ? Math.max(0, x - 16 - width) : x + 16;
    tip.style.left = `${left}px`;
    tip.style.top = `${Math.max(0, y - tip.offsetHeight - 8)}px`;
  };

  document.addEventListener("pointermove", (event) => {
    const hit = event.target instanceof Element && event.target.closest(".chart .hits rect");
    if (hit) {
      show(hit, event);
    } else {
      hideTips(null);
    }
  });

  const copied = (button, text) => {
    const label = button.dataset.label || button.textContent;
    button.dataset.label = label;
    button.textContent = text;
    setTimeout(() => {
      button.textContent = label;
    }, 1600);
  };

  document.addEventListener("click", (event) => {
    const button = event.target instanceof Element && event.target.closest("[data-copy]");
    if (!button) {
      return;
    }
    const source = document.getElementById(button.dataset.copy);
    if (!source) {
      return;
    }
    const fallback = () => {
      const range = document.createRange();
      range.selectNodeContents(source);
      const selection = getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      let done = false;
      try {
        done = document.execCommand("copy");
      } catch {
        done = false;
      }
      copied(button, done ? "Copied" : "Selected, press Ctrl+C");
    };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(source.textContent).then(() => copied(button, "Copied"), fallback);
    } else {
      fallback();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey) {
      return;
    }
    const target = event.target;
    if (target instanceof Element && target.closest("input, textarea, select, [contenteditable]")) {
      return;
    }
    const search = document.querySelector("[data-search]");
    if (search) {
      event.preventDefault();
      search.focus();
      search.select();
    }
  });
})();
