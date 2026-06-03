// === Luna Shell — Navigation Manager ===
// 2D spatial D-pad navigation: computes visual grid from element positions.
// Arrow keys move by row/column, Enter/Space clicks.
// Works on any screen layout — home, settings, about, app-overlay.

class NavigationManager {
  constructor() {
    this.focusIndex = 0;
    this.focusableSelector =
      '.card, .nav-item, .btn, .toggle, .notif-toast, .setting-row, input[type="range"]';
    this.grid = [];       // flat index → { el, row, col }
    this.rows = [];       // row index → [flatIndex, ...]
    this.cols = [];       // col index → [flatIndex, ...] (sorted by row)
    this.onActivate = null;
    this._keyHandler = this._onKey.bind(this);
  }

  start() {
    document.addEventListener('keydown', this._keyHandler);
  }
  stop() {
    document.removeEventListener('keydown', this._keyHandler);
  }

  /** Rebuild spatial grid from visible focusable elements. */
  refresh() {
    const activeScreen = this._activeScreen();
    if (!activeScreen) return;

    const els = Array.from(activeScreen.querySelectorAll(this.focusableSelector))
      .filter(el => el.offsetParent !== null); // only visible

    // Auto-add tabindex for non-focusable elements (divs, spans, etc.)
    for (const el of els) {
      if (!el.hasAttribute('tabindex') && el.tagName !== 'INPUT' && el.tagName !== 'BUTTON') {
        el.setAttribute('tabindex', '-1');
      }
    }

    if (els.length === 0) return;

    // Build spatial rows: group elements by vertical position (within 24px)
    const sorted = els.map(el => ({ el, rect: el.getBoundingClientRect() }));
    sorted.sort((a, b) => a.rect.top - b.rect.top || a.rect.left - b.rect.left);

    const rawRows = [];
    for (const item of sorted) {
      const lastRow = rawRows[rawRows.length - 1];
      if (lastRow && Math.abs(item.rect.top - lastRow[0].rect.top) < 24) {
        lastRow.push(item);
      } else {
        rawRows.push([item]);
      }
    }

    // Sort each row by column (left position)
    for (const row of rawRows) {
      row.sort((a, b) => a.rect.left - b.rect.left);
    }

    // Build grid: assign (row, col) to every element
    this.grid = [];
    this.rows = [];
    this.cols = [];
    for (let r = 0; r < rawRows.length; r++) {
      const rowEls = rawRows[r];
      this.rows[r] = [];
      for (let c = 0; c < rowEls.length; c++) {
        const flatIdx = this.grid.length;
        this.grid.push({ el: rowEls[c].el, row: r, col: c });
        this.rows[r].push(flatIdx);
        if (!this.cols[c]) this.cols[c] = [];
        this.cols[c].push(flatIdx);
      }
    }

    // Clamp focusIndex
    if (this.focusIndex >= this.grid.length) this.focusIndex = 0;
    this._applyFocus();
  }

  _activeScreen() {
    // Check: app overlay first, then notification overlay, then active screen, then body
    const overlay = document.getElementById('app-overlay');
    if (overlay && overlay.classList.contains('active')) return overlay;
    const notif = document.getElementById('notif-overlay');
    if (notif && notif.classList.contains('active')) return notif;
    return document.querySelector('.screen.active') || document.body;
  }

  setFocus(index) {
    if (index >= 0 && index < this.grid.length) {
      this.focusIndex = index;
      this._applyFocus();
    }
  }

  _applyFocus() {
    this.grid.forEach((item, i) => {
      if (i === this.focusIndex) {
        item.el.classList.add('focused');
        if (item.el.focus) item.el.focus({ preventScroll: true });
        item.el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      } else {
        item.el.classList.remove('focused');
      }
    });
  }

  /** Find best index in targetRow near targetCol. */
  _closestInRow(targetRow, targetCol) {
    if (!this.rows[targetRow] || this.rows[targetRow].length === 0) return -1;
    const candidates = this.rows[targetRow];
    let best = candidates[0];
    let bestDist = Infinity;
    for (const idx of candidates) {
      const dist = Math.abs(this.grid[idx].col - targetCol);
      if (dist < bestDist) { best = idx; bestDist = dist; }
    }
    return best;
  }

  _onKey(e) {
    if (this.grid.length === 0) return;

    const cur = this.grid[this.focusIndex];
    if (!cur) return;

    // Slider special handling
    if (cur.el.tagName === 'INPUT' && cur.el.type === 'range') {
      this._handleSliderKey(cur.el, e);
      return;
    }

    switch (e.key) {
      case 'ArrowRight': {
        e.preventDefault();
        // Try next in same row
        const row = this.rows[cur.row];
        const colIdx = row.indexOf(this.focusIndex);
        if (colIdx < row.length - 1) {
          this.focusIndex = row[colIdx + 1];
        } else {
          // Wrap to next row
          const nextRow = (cur.row + 1) % this.rows.length;
          this.focusIndex = this._closestInRow(nextRow, 0);
        }
        this._applyFocus();
        break;
      }
      case 'ArrowLeft': {
        e.preventDefault();
        const row = this.rows[cur.row];
        const colIdx = row.indexOf(this.focusIndex);
        if (colIdx > 0) {
          this.focusIndex = row[colIdx - 1];
        } else {
          // Wrap to previous row, last column
          const prevRow = (cur.row - 1 + this.rows.length) % this.rows.length;
          this.focusIndex = this._closestInRow(prevRow, 999);
        }
        this._applyFocus();
        break;
      }
      case 'ArrowDown': {
        e.preventDefault();
        // Try same column in next row
        const nextRow = (cur.row + 1) % this.rows.length;
        this.focusIndex = this._closestInRow(nextRow, cur.col);
        this._applyFocus();
        break;
      }
      case 'ArrowUp': {
        e.preventDefault();
        const prevRow = (cur.row - 1 + this.rows.length) % this.rows.length;
        this.focusIndex = this._closestInRow(prevRow, cur.col);
        this._applyFocus();
        break;
      }
      case 'Enter':
      case ' ': {
        e.preventDefault();
        if (cur.el.tagName === 'INPUT' && cur.el.type === 'range') return;
        // For setting-row: click the toggle/button inside
        if (cur.el.classList.contains('setting-row')) {
          const interactive = cur.el.querySelector('.toggle, button, input');
          if (interactive) interactive.click();
          else cur.el.click();
        } else {
          cur.el.click();
        }
        break;
      }
      case 'Escape': {
        // Close app overlay if open
        const appOverlay = document.getElementById('app-overlay');
        if (appOverlay && appOverlay.classList.contains('active')) {
          window.luna._closeApp();
          break;
        }
        // Close notification overlay if open
        const notifEl = document.getElementById('notif-overlay');
        if (notifEl && notifEl.classList.contains('active')) {
          notifEl.classList.remove('active');
          setTimeout(() => this.refresh(), 50);
          break;
        }
        // Go home
        if (this.onActivate) this.onActivate('screen:home', {});
        break;
      }
      case 'n': {
        if (e.ctrlKey || e.metaKey) break;
        const notifEl = document.getElementById('notif-overlay');
        if (notifEl) notifEl.classList.toggle('active');
        setTimeout(() => this.refresh(), 50);
        break;
      }
    }
  }

  _handleSliderKey(slider, e) {
    const step = parseFloat(slider.step) || 1;
    const min = parseFloat(slider.min) || 0;
    const max = parseFloat(slider.max) || 100;
    let val = parseFloat(slider.value);

    switch (e.key) {
      case 'ArrowRight':
      case 'ArrowUp':
        e.preventDefault();
        val = Math.min(max, val + step);
        slider.value = val;
        slider.dispatchEvent(new Event('input', { bubbles: true }));
        slider.dispatchEvent(new Event('change', { bubbles: true }));
        break;
      case 'ArrowLeft':
      case 'ArrowDown':
        e.preventDefault();
        val = Math.max(min, val - step);
        slider.value = val;
        slider.dispatchEvent(new Event('input', { bubbles: true }));
        slider.dispatchEvent(new Event('change', { bubbles: true }));
        break;
      case 'Enter':
      case ' ':
        // Exit slider mode, move to next element
        e.preventDefault();
        this.focusIndex = (this.focusIndex + 1) % this.grid.length;
        this._applyFocus();
        break;
    }
  }
}

const nav = new NavigationManager();
export default nav;
