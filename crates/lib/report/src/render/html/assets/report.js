(() => {
  const activateTab = (button) => {
    const set = button.closest("[data-tab-set]");
    if (!set) return;

    const setId = set.dataset.tabSet;
    const target = document.getElementById(button.dataset.tabTarget);
    if (!target) return;

    for (const candidate of set.querySelectorAll(".tab-button")) {
      if (candidate.dataset.tabParent === setId) {
        candidate.classList.toggle("active", candidate === button);
        candidate.setAttribute("aria-selected", candidate === button ? "true" : "false");
      }
    }

    for (const panel of set.querySelectorAll(".tab-panel")) {
      if (panel.dataset.tabParent === setId) {
        panel.classList.toggle("active", panel === target);
      }
    }
  };

  for (const button of document.querySelectorAll(".tab-button")) {
    button.addEventListener("click", () => activateTab(button));
  }

  const themeButton = document.querySelector(".theme-toggle");
  if (themeButton) {
    const root = document.documentElement;
    const preference = matchMedia("(prefers-color-scheme: dark)");
    const storageKey = "rust-glancer-report-theme";
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved === "light" || saved === "dark") root.dataset.theme = saved;
    } catch {
      // Browsers can disable storage for reports opened directly from disk.
    }
    const currentTheme = () => root.dataset.theme ?? (preference.matches ? "dark" : "light");
    const updateButton = () => {
      const dark = currentTheme() === "dark";
      const label = dark ? "Switch to light theme" : "Switch to dark theme";
      themeButton.textContent = dark ? "☀" : "☾";
      themeButton.title = label;
      themeButton.setAttribute("aria-label", label);
    };
    themeButton.addEventListener("click", () => {
      const next = currentTheme() === "dark" ? "light" : "dark";
      root.dataset.theme = next;
      try {
        localStorage.setItem(storageKey, next);
      } catch {
        // The override still applies to the current report.
      }
      updateButton();
    });
    preference.addEventListener("change", updateButton);
    updateButton();
    themeButton.hidden = false;
  }

  for (const block of document.querySelectorAll(".table-content")) {
    const tbody = block.querySelector("tbody");
    const search = block.querySelector(".table-filter");
    const filters = Array.from(block.querySelectorAll("[data-filter-column]"));
    const pager = block.querySelector(".table-pages");
    const previous = pager?.querySelector('[data-page="prev"]');
    const next = pager?.querySelector('[data-page="next"]');
    const pageSize = pager ? Number(pager.dataset.pageSize) : Infinity;
    let page = 0;

    // Reorder whole comparison groups. The first side is the sort key, and the original
    // within-group order is shared by the default view, filtering, and pagination.
    const byKey = new Map();
    for (const row of tbody.rows) {
      const key = row.dataset.rowGroup ?? row;
      if (!byKey.has(key)) byKey.set(key, []);
      byKey.get(key).push(row);
    }
    const groups = Array.from(byKey.values());
    const rowMatches = (row, needle) =>
      (!needle || row.textContent.toLowerCase().includes(needle)) &&
      filters.every((filter) => !filter.value ||
        row.cells[Number(filter.dataset.filterColumn)]?.dataset.filterValue === filter.value);
    const update = () => {
      const needle = search?.value.trim().toLowerCase();
      const matching = groups.filter((group) => group.some((row) => rowMatches(row, needle)));
      const count = matching.length;
      const last = pager ? Math.max(0, Math.ceil(count / pageSize) - 1) : 0;
      page = Math.min(Math.max(0, page), last);
      const displayed = pager ? matching.slice(page * pageSize, (page + 1) * pageSize) : matching;
      const visible = new Map(displayed.map((group, index) => [group, index % 2 === 1]));
      for (const group of groups) {
        for (const row of group) {
          row.hidden = !visible.has(group);
          row.classList.toggle("is-striped", visible.get(group) === true);
        }
      }
      if (pager) {
        previous.disabled = page === 0;
        next.disabled = page === last;
        pager.querySelector(".page-summary").textContent = count
          ? `${page + 1} / ${last + 1}`
          : "No matching rows";
      }
    };
    const resetPage = () => {
      page = 0;
      update();
    };
    search?.addEventListener("input", resetPage);
    for (const filter of filters) filter.addEventListener("change", resetPage);
    previous?.addEventListener("click", () => { page--; update(); });
    next?.addEventListener("click", () => { page++; update(); });

    const headers = block.querySelectorAll("th.sortable");
    for (const header of headers) {
      header.addEventListener("click", (event) => {
        if (event.target.closest(".tab-help")) return;
        const index = Number(header.dataset.columnIndex);
        const direction = header.dataset.sortDir === "asc" ? "desc" : "asc";
        for (const candidate of headers) {
          candidate.removeAttribute("data-sort-dir");
          candidate.removeAttribute("aria-sort");
        }
        header.dataset.sortDir = direction;
        header.setAttribute("aria-sort", direction === "asc" ? "ascending" : "descending");
        groups.sort(([left], [right]) => {
          const a = left.cells[index]?.dataset.sort ?? "";
          const b = right.cells[index]?.dataset.sort ?? "";
          if (!a || !b) return Number(!a) - Number(!b);
          const aNumber = Number(a);
          const bNumber = Number(b);
          const order = Number.isFinite(aNumber) && Number.isFinite(bNumber)
            ? aNumber - bNumber
            : a.localeCompare(b);
          return direction === "asc" ? order : -order;
        });
        tbody.append(...groups.flat());
        resetPage();
      });
    }

    update();
  }
})();
