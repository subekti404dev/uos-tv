// === Luna Shell — App Controller ===
// Orchestrates screens, events, and system state

import bus from './bus.js';
import nav from './nav.js';

class LunaApp {
  constructor() {
    this.currentScreen = 'boot';
    this.volume = 70;
    this.muted = false;
    this.brightness = 80;
    this.nightMode = false;
    this.wifiConnected = true;
    this.notifications = [];
    this.booting = true;
  }

  async init() {
    console.log('[Luna] Initializing UOS TV Shell...');

    // Connect to stardust bus
    await bus.connect();

    // Subscribe to system events
    bus.subscribe('audio.status', (msg) => this._onAudioStatus(msg.params));
    bus.subscribe('network.status', (msg) => this._onNetworkStatus(msg.params));
    bus.subscribe('power.status', (msg) => this._onPowerStatus(msg.params));
    bus.subscribe('display.status', (msg) => this._onDisplayStatus(msg.params));
    bus.subscribe('notification.list', (msg) => this._onNotification(msg));
    bus.subscribe('pkg.*', (msg) => this._onPkgEvent(msg));

    // Start navigation
    nav.onActivate = (action, data) => this._onNavigate(action, data);
    nav.start();

    // Boot sequence
    this._showBoot();
    await this._delay(2500);
    this._hideBoot();
    this.booting = false;

    // Show home
    this.showScreen('home');
    nav.refresh();

    // Start clock
    this._startClock();
    this._updateStatusBar();

    console.log('[Luna] Ready.');
  }

  // === Screen Management ===
  showScreen(name) {
    document.querySelectorAll('.screen').forEach(s => s.classList.remove('active'));
    const screen = document.getElementById(`screen-${name}`);
    if (screen) {
      screen.classList.add('active');
      this.currentScreen = name;
      // Slight delay: browsers batch display changes, refresh after paint
      requestAnimationFrame(() => {
        requestAnimationFrame(() => nav.refresh());
      });
    }
  }

  // === Boot ===
  _showBoot() {
    document.getElementById('boot-overlay').classList.remove('hidden');
  }

  _hideBoot() {
    const overlay = document.getElementById('boot-overlay');
    overlay.classList.add('hiding');
    setTimeout(() => overlay.classList.add('hidden'), 900);
  }

  // === Navigation Handler ===
  _onNavigate(action, data) {
    switch (action) {
      case 'screen:home':
        this.showScreen('home');
        break;
      case 'screen:settings':
        this.showScreen('settings');
        this._showSettingsTab('network');
        break;
      case 'screen:about':
        this.showScreen('about');
        break;
      case 'launch:app':
        this._launchApp(data.id, data.name);
        break;
      case 'toggle:wifi':
        this.wifiConnected = !this.wifiConnected;
        this._updateStatusBar();
        break;
    }
  }

  // === App Launching ===
  _launchApp(id, name) {
    console.log(`[Luna] Launching: ${name} (${id})`);

    // In production: call pkgd via stardust to launch app
    bus.publish('pkg.command.launch', { id, name });

    // Show mock app overlay (dev mode)
    const overlay = document.getElementById('app-overlay');
    const appIcon = document.getElementById('app-overlay-icon');
    const appName = document.getElementById('app-overlay-name');
    const appId = document.getElementById('app-overlay-id');

    const icons = {
      'YouTube': '▶️', 'Netflix': '🎬', 'Spotify': '🎵',
      'Plex': '📁', 'Twitch': '🎮', 'Disney+': '✨'
    };
    if (appIcon) appIcon.textContent = icons[name] || '📦';
    if (appName) appName.textContent = name;
    if (appId) appId.textContent = id;
    if (overlay) overlay.classList.add('active');
  }

  _closeApp() {
    const overlay = document.getElementById('app-overlay');
    if (overlay) overlay.classList.remove('active');
    setTimeout(() => nav.refresh(), 50);
  }

  // === Settings ===
  _showSettingsTab(tab) {
    document.querySelectorAll('.settings-nav .nav-item').forEach(n => n.classList.remove('active'));
    document.querySelectorAll('.settings-panel').forEach(p => p.classList.remove('active'));

    const navItem = document.querySelector(`.settings-nav [data-tab="${tab}"]`);
    const panel = document.getElementById(`settings-${tab}`);

    if (navItem) navItem.classList.add('active');
    if (panel) panel.classList.add('active');

    requestAnimationFrame(() => {
      requestAnimationFrame(() => nav.refresh());
    });
  }

  // === Audio ===
  setVolume(vol) {
    this.volume = Math.max(0, Math.min(100, Math.round(vol)));
    this._showVolumeHUD();
    bus.publish('audio.command.set_volume', { volume: this.volume });
    const slider = document.getElementById('vol-slider');
    if (slider) slider.value = this.volume;
  }

  volumeUp() {
    this.setVolume(this.volume + 5);
  }

  volumeDown() {
    this.setVolume(this.volume - 5);
  }

  toggleMute() {
    this.muted = !this.muted;
    bus.publish('audio.command.mute', {});
    if (this.muted) {
      this._showVolumeHUD(true);
    } else {
      this._showVolumeHUD();
    }
  }

  _showVolumeHUD(muted = false) {
    const hud = document.getElementById('volume-hud');
    const icon = document.getElementById('vol-icon');
    const fill = document.getElementById('vol-fill');
    const label = document.getElementById('vol-label');

    if (muted || this.volume === 0) {
      icon.textContent = '🔇';
      fill.style.width = '0%';
      label.textContent = 'Muted';
    } else if (this.volume < 30) {
      icon.textContent = '🔈';
      fill.style.width = `${this.volume}%`;
      label.textContent = `${this.volume}%`;
    } else if (this.volume < 70) {
      icon.textContent = '🔉';
      fill.style.width = `${this.volume}%`;
      label.textContent = `${this.volume}%`;
    } else {
      icon.textContent = '🔊';
      fill.style.width = `${this.volume}%`;
      label.textContent = `${this.volume}%`;
    }

    hud.classList.add('showing');
    hud.classList.remove('hiding');

    clearTimeout(this._volHudTimer);
    this._volHudTimer = setTimeout(() => {
      hud.classList.add('hiding');
      setTimeout(() => hud.classList.remove('showing', 'hiding'), 300);
    }, 1500);
  }

  // === Notifications ===
  _showToast(message, priority = 'normal') {
    const toast = {
      id: Date.now().toString(),
      app: 'luna',
      title: 'UOS TV',
      body: message,
      priority,
      timestamp: Date.now(),
    };

    this.notifications.unshift(toast);
    if (this.notifications.length > 10) this.notifications.pop();
    this._renderToasts();

    // Auto-dismiss
    setTimeout(() => this._dismissToast(toast.id), 4000);
  }

  _dismissToast(id) {
    const el = document.querySelector(`.notif-toast[data-id="${id}"]`);
    if (el) {
      el.classList.add('hiding');
      setTimeout(() => {
        this.notifications = this.notifications.filter(n => n.id !== id);
        this._renderToasts();
      }, 250);
    }
  }

  _renderToasts() {
    const container = document.getElementById('notif-list');
    if (!container) return;

    container.innerHTML = this.notifications.map(n => `
      <div class="notif-toast ${n.priority}" data-id="${n.id}" onclick="window.luna._dismissToast('${n.id}')">
        <div class="notif-header">
          <span class="notif-title">${n.title}</span>
          <span class="notif-time">${new Date(n.timestamp).toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'})}</span>
        </div>
        <div class="notif-body">${n.body}</div>
      </div>
    `).join('');

    const overlay = document.getElementById('notif-overlay');
    if (this.notifications.length > 0) {
      overlay.classList.add('active');
      // Auto-hide overlay after 5s of no new notifications
      clearTimeout(this._notifTimer);
      this._notifTimer = setTimeout(() => overlay.classList.remove('active'), 5000);
    } else {
      overlay.classList.remove('active');
    }
  }

  // === Status Bar ===
  _updateStatusBar() {
    // WiFi icon
    const wifiEl = document.getElementById('status-wifi');
    if (wifiEl) {
      wifiEl.textContent = this.wifiConnected ? '📶' : '📶';
      wifiEl.classList.toggle('active', this.wifiConnected);
    }

    // Volume icon
    const volEl = document.getElementById('status-vol');
    if (volEl) {
      if (this.muted || this.volume === 0) {
        volEl.textContent = '🔇';
      } else if (this.volume < 30) volEl.textContent = '🔈';
      else if (this.volume < 70) volEl.textContent = '🔉';
      else volEl.textContent = '🔊';
    }
  }

  // === Event Handlers ===
  _onAudioStatus(data) {
    if (data.volume !== undefined) this.volume = data.volume;
    if (data.muted !== undefined) this.muted = data.muted;
    this._updateStatusBar();
  }

  _onNetworkStatus(data) {
    this.wifiConnected = data.connected;
    this._updateStatusBar();
  }

  _onPowerStatus(data) {
    // Handle dimmed/suspended states
    if (data.state === 'dimmed') {
      document.body.style.filter = 'brightness(0.7)';
    } else {
      document.body.style.filter = '';
    }
  }

  _onDisplayStatus(data) {
    if (data.brightness !== undefined) this.brightness = data.brightness;
    if (data.night_mode !== undefined) this.nightMode = data.night_mode;
  }

  _onNotification(msg) {
    // Handle notification.list payload: { notifications: [...], count: N }
    if (msg.params) {
      const params = msg.params;
      if (params.notifications && Array.isArray(params.notifications)) {
        params.notifications.forEach(n => {
          this._showToast(n.body || n.title, n.priority || 'normal');
        });
      } else if (params.body) {
        this._showToast(params.body, params.priority || 'normal');
      }
    }
  }

  _onPkgEvent(msg) {
    console.log('[Luna] Package event:', msg.method, msg.params);
  }

  // === Clock ===
  _startClock() {
    const tick = () => {
      const el = document.getElementById('topbar-time');
      if (el) {
        const now = new Date();
        el.textContent = now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
      }
      if (this.booting) return; // Don't keep ticking after boot
    };
    tick();
    setInterval(tick, 15000);
  }

  _delay(ms) {
    return new Promise(r => setTimeout(r, ms));
  }
}

// Create singleton on window for HTML onclick handlers
const app = new LunaApp();
window.luna = app;

// Boot when DOM ready
document.addEventListener('DOMContentLoaded', () => app.init());
export default app;
