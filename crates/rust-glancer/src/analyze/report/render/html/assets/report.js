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

  for (const input of document.querySelectorAll(".table-filter")) {
    input.addEventListener("input", () => {
      const tableBlock = input.closest(".table-content");
      if (!tableBlock) return;
      const needle = input.value.trim().toLowerCase();
      for (const row of tableBlock.querySelectorAll("tbody tr")) {
        row.hidden = needle !== "" && !row.textContent.toLowerCase().includes(needle);
      }
    });
  }

  for (const header of document.querySelectorAll("th.sortable")) {
    header.addEventListener("click", () => {
      const table = header.closest("table");
      if (!table) return;
      const tbody = table.querySelector("tbody");
      if (!tbody) return;
      const index = Number(header.dataset.columnIndex);
      const direction = header.dataset.sortDir === "asc" ? "desc" : "asc";

      for (const candidate of table.querySelectorAll("th.sortable")) {
        candidate.removeAttribute("data-sort-dir");
      }
      header.dataset.sortDir = direction;

      const rows = Array.from(tbody.querySelectorAll("tr"));
      rows.sort((left, right) => {
        const leftValue = left.children[index]?.dataset.sort ?? "";
        const rightValue = right.children[index]?.dataset.sort ?? "";
        const leftNumber = Number(leftValue);
        const rightNumber = Number(rightValue);
        const comparison = Number.isFinite(leftNumber) && Number.isFinite(rightNumber)
          ? leftNumber - rightNumber
          : leftValue.localeCompare(rightValue);
        return direction === "asc" ? comparison : -comparison;
      });

      for (const row of rows) {
        tbody.appendChild(row);
      }
    });
  }
})();
