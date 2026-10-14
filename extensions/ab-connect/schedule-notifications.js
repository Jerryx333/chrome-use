// Notify on a persisted terminal run, never on an accepted prompt or tool call.
// The cursor stores IDs and timestamps only, not prompts or transcript text.
export class ScheduleNotifications {
  constructor({ storage, notifications, now = Date.now }) {
    this.storage = storage; this.notifications = notifications; this.now = now;
    this.startedAt = now(); this.seen = null; this.running = false;
  }
  async poll(read) {
    if (this.running) return;
    this.running = true;
    try {
      if (!this.seen) this.seen = (await this.storage.get('ab_schedule_notifications')).ab_schedule_notifications || {};
      const { schedules } = await read();
      const live = new Set();
      for (const task of schedules) {
        live.add(task.id);
        if (!task.finishedAt || !task.lastChat || !task.lastResult || task.activeChat) continue;
        const token = `${task.lastChat}:${task.finishedAt}`;
        if (this.seen[task.id] === token) continue;
        if (task.finishedAt >= this.startedAt || this.seen[task.id]) {
          await this.notifications.create(`schedule-run:${task.lastChat}`, { type: 'basic', iconUrl: 'icons/icon128.png', title: task.name.slice(0, 120), message: task.lastResult.slice(0, 240) });
        }
        this.seen[task.id] = token;
      }
      // Keep cursors from other Chrome profiles; a host switch is not deletion.
      if (Object.keys(this.seen).length > 256) this.seen = Object.fromEntries(Object.entries(this.seen).slice(-256));
      await this.storage.set({ ab_schedule_notifications: this.seen });
    } finally { this.running = false; }
  }
}
