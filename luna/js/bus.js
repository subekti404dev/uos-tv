// === Luna Shell — Stardust IPC Client (WebSocket) ===
//
// Connects to stardust via WebSocket bridge at ws://127.0.0.1:9090
// Protocol: JSON messages — see stardust/src/ws.rs for details

class StardustClient {
  constructor() {
    this.ws = null;
    this.subscribers = new Map();
    this.pendingCalls = new Map();
    this.serviceName = 'luna-ui';
    this.connected = false;
    this.reconnectDelay = 1000;
    this.maxReconnectDelay = 30000;
    this.wsUrl = 'ws://127.0.0.1:9090';
  }

  async connect() {
    return new Promise((resolve) => {
      try {
        this.ws = new WebSocket(this.wsUrl);
      } catch (e) {
        console.warn('[Stardust] WebSocket not available, using fallback');
        this._setupFallback();
        resolve(false);
        return;
      }

      this.ws.onopen = () => {
        console.log('[Stardust] Connected to', this.wsUrl);
        this.connected = true;
        this.reconnectDelay = 1000;
        resolve(true);
      };

      this.ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
          this._dispatch(msg);
        } catch (e) {
          console.warn('[Stardust] Invalid message:', e);
        }
      };

      this.ws.onclose = () => {
        console.log('[Stardust] Disconnected');
        this.connected = false;
        this._reconnect();
      };

      this.ws.onerror = (e) => {
        console.warn('[Stardust] WebSocket error, using fallback');
        this._setupFallback();
        resolve(false);
      };

      // Timeout fallback
      setTimeout(() => {
        if (!this.connected) {
          this._setupFallback();
          resolve(false);
        }
      }, 2000);
    });
  }

  _setupFallback() {
    console.log('[Stardust] Running in simulated mode');
    this.connected = true;
  }

  _reconnect() {
    setTimeout(() => {
      console.log('[Stardust] Reconnecting...');
      this.connect();
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
    }, this.reconnectDelay);
  }

  _dispatch(msg) {
    if (msg.type === 'event') {
      for (const [pattern, callbacks] of this.subscribers) {
        if (this._matchTopic(pattern, msg.method)) {
          for (const cb of callbacks) {
            try { cb({ method: msg.method, params: msg.params }); } catch (e) {}
          }
        }
      }
    } else if (msg.type === 'response') {
      const resolve = this.pendingCalls.get(msg.id);
      if (resolve) {
        this.pendingCalls.delete(msg.id);
        resolve(msg.data);
      }
    } else if (msg.type === 'error') {
      const reject = this.pendingCalls.get(msg.id);
      if (reject) {
        this.pendingCalls.delete(msg.id);
      }
    }
  }

  subscribe(pattern, callback) {
    if (!this.subscribers.has(pattern)) {
      this.subscribers.set(pattern, []);
    }
    this.subscribers.get(pattern).push(callback);

    if (this.ws && this.connected) {
      this.ws.send(JSON.stringify({ type: 'subscribe', topic: pattern }));
    }
  }

  unsubscribe(pattern) {
    this.subscribers.delete(pattern);
    if (this.ws && this.connected) {
      this.ws.send(JSON.stringify({ type: 'unsubscribe', topic: pattern }));
    }
  }

  publish(method, params = {}) {
    if (this.ws && this.connected) {
      this.ws.send(JSON.stringify({ type: 'publish', method, params }));
    } else {
      console.log('[Stardust/sim] Publish:', method, params);
    }
  }

  async call(method, params = {}) {
    if (!this.ws || !this.connected) {
      return { status: 'ok', data: {} };
    }

    const id = crypto.randomUUID ? crypto.randomUUID() : Date.now().toString();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingCalls.delete(id);
        reject(new Error('call timeout'));
      }, 5000);

      this.pendingCalls.set(id, (data) => {
        clearTimeout(timer);
        resolve(data);
      });

      this.ws.send(JSON.stringify({ type: 'call', method, params, id }));
    });
  }

  _matchTopic(pattern, topic) {
    if (pattern === topic) return true;
    if (pattern.endsWith('.*')) {
      return topic.startsWith(pattern.slice(0, -2));
    }
    return false;
  }
}

const bus = new StardustClient();
export default bus;
