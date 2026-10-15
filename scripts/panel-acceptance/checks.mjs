// The live checks of plan 14.2 (1-20) plus the real-backend runs. Each
// run(h, ev) asserts and fills ev with the evidence results.json keeps.
// A check returns {status: 'partial', note} when part of it cannot be run on
// this build (the note says which part and why); anything else is pass/fail.
import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { pdfBody } from './site/server.mjs';
import { Service } from './lib.mjs';

const expectPredicate = a => [a.not ? 'not' : null, a.kind, a.selector, a.state, a.attr, a.op, a.count, a.predicate, a.expected, a.filter, a.method, a.status, a.type, a.requestCount].filter(v => v != null).join(' ');
const turnOf = (h, events, messageId) => ({
  end: h.one(events, 'turnEnded', d => d.messageId === messageId)?.data,
  final: h.finalAssessment(events, messageId),
  actions: h.actions(events, messageId),
  stalls: events.filter(e => e.kind === 'progressStalled' && e.data.messageId === messageId).map(e => e.data),
});
const brief = a => ({ actionId: a.actionId, tool: a.admitted?.tool, outcome: a.target?.outcome, errorCode: a.target?.errorCode, verdict: a.observed?.verdict, basis: a.observed?.basis, possiblyExecuted: a.observed?.possiblyExecuted, afterStop: a.target?.afterStop, afterTurnEnd: a.target?.afterTurnEnd, lateReported: a.target?.lateReported, before: a.target?.observed?.before && { url: a.target.observed.before.url, provenance: a.target.observed.before.provenance }, after: a.target?.observed?.after && { url: a.target.observed.after.url, chromeTabId: a.target.observed.after.chromeTabId, provenance: a.target.observed.after.provenance }, requested: a.target?.requested, detail: a.observed?.detail });
// Map-aware structural dump of a PanelTimeline for deep comparison.
const plain = v => v instanceof Map ? [...v.entries()].map(([k, x]) => [k, plain(x)]) : Array.isArray(v) ? v.map(plain) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, plain(x)])) : v;
const outputsOf = (events, messageId) => events.filter(e => e.kind === 'actionAdmitted' && e.data.messageId === messageId).flatMap(e => (e.data.outputs || []).map(o => ({ data: { ...o, messageId, actionId: e.data.actionId } })));
const artifactEvents = (events, artifactId) => events.filter(e => e.data?.artifactId === artifactId).map(e => ({ kind: e.kind, sequence: e.sequence, state: e.data.state, reason: e.data.reason, mime: e.data.mime, bytes: e.data.bytes, digest: e.data.digest, typeMatches: e.data.typeMatches, detectedBy: e.data.detectedBy, messageId: e.data.messageId }));

export const checks = [
  {
    id: 1, name: 'Tool failure then recovery', timeout: 180,
    async run(h, ev) {
      const conv = await h.create('codex');
      const expectArgs = { kind: 'text', selector: '#status', predicate: 'contains', expected: 'Draft saved' };
      const text = h.scenario('c1', { steps: [
        { say: 'Opening the form.', role: 'commentary' },
        { tool: 'chrome_use_open', args: { url: h.site('/form') } },
        { tool: 'chrome_use_fill', args: { selector: '#name', value: 'Alice' } },
        { tool: 'chrome_use_click', args: { selector: '#does-not-exist', timeoutMs: 3000 }, save: 'bad' },
        { tool: 'chrome_use_click', args: { selector: '#save-ok', observe: true }, save: 'good' },
        { tool: 'chrome_use_expect', args: expectArgs, save: 'e1' },
        { assess: { status: 'complete', requirements: [{ text: 'Save the draft', status: 'met', check: { kind: 'expect', predicate: expectPredicate(expectArgs), evidence: '${e1.actionId}' } }] } },
        { say: 'Saved the draft after the first selector failed.' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const t = turnOf(h, events, messageId);
      ev.actions = t.actions.map(brief);
      ev.turnEnded = t.end; ev.assessment = t.final;
      const tools = h.kinds(events, 'tool').filter(e => e.data.completed);
      ev.toolSteps = tools.map(e => ({ toolCallId: e.data.toolCallId, actionIds: e.data.actionIds }));
      const failedClick = t.actions.find(a => a.admitted?.tool === 'chrome_use_click' && a.observed?.verdict === 'failed');
      h.assert.ok(failedClick, 'the failing click is journaled with a failed verdict');
      h.assert.ok(tools.some(e => e.data.actionIds?.includes(failedClick.actionId)), 'the failed step is linked to its action');
      h.assert.ok(t.actions.filter(a => a.observed?.verdict === 'failed').length === 1, 'only the bad click failed');
      h.assert.equal(t.end.outcome, 'completed');
      h.assert.equal(t.final.status, 'checksPassed', 'one requirement checked by a matching expect');
      ev.panel = await h.screenshot('check1', conv.id);
      // The folded step group names the failed step (plan 9.2.3).
      h.panelCheck(ev, /1 failed: Browser · click/i.test(ev.panel.transcript), 'the folded step group names the failed step');
      h.panelCheck(ev, /Agent's browser checks passed/.test(ev.panel.transcript) && /Completed/.test(ev.panel.transcript), 'badge and outcome line');
      h.state.c1 = { conv, messageId };
    },
  },
  {
    id: 2, name: 'Empty answer', timeout: 120,
    async run(h, ev) {
      const conv = await h.create('codex');
      const text = h.scenario('c2', { steps: [{ tool: 'chrome_use_open', args: { url: h.site('/a') } }, { tool: 'chrome_use_read', args: {} }] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const t = turnOf(h, events, messageId);
      ev.turnEnded = t.end; ev.assessment = t.final;
      h.assert.equal(t.end.outcome, 'completed');
      h.assert.equal(t.end.emptyAnswer, true);
      h.assert.equal(t.final.source, 'host');
      h.assert.equal(t.final.status, 'notChecked');
      h.assert.equal(t.final.reason, 'notAssessed');
      ev.panel = await h.screenshot('check2', conv.id);
      h.panelCheck(ev, /no answer text/i.test(ev.panel.text), 'panel shows /no answer text/i');
    },
  },
  {
    id: 3, name: 'Partial completion', timeout: 120,
    async run(h, ev) {
      const conv = await h.create('codex');
      const e1 = { kind: 'text', selector: '#title', predicate: 'contains', expected: 'Page A' };
      const e2 = { kind: 'text', selector: '#title', predicate: 'contains', expected: 'Page' };
      const text = h.scenario('c3', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/a') } },
        { tool: 'chrome_use_expect', args: e1, save: 'e1' },
        { tool: 'chrome_use_expect', args: e2, save: 'e2' },
        { assess: { status: 'complete', reason: 'done', requirements: [
          { text: 'Open page A', status: 'met', check: { kind: 'expect', predicate: expectPredicate(e1), evidence: '${e1.actionId}' } },
          { text: 'Place the order', status: 'met' },
          { text: 'Totals equal 42', status: 'met', check: { kind: 'expect', predicate: 'text #total contains 42', evidence: '${e2.actionId}' } },
        ] } },
        { say: 'Done.' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const t = turnOf(h, events, messageId);
      ev.assessment = t.final;
      h.assert.equal(t.final.status, 'someChecksPassed');
      h.assert.equal(t.final.declaredStatus, 'complete');
      h.assert.deepEqual(t.final.requirements.map(r => r.computed), ['checked', 'claimed', 'checkFailed']);
      h.assert.ok(t.final.downgradedBecause.length >= 1, 'the done declaration is downgraded with reasons');
      ev.panel = await h.screenshot('check3', conv.id);
      h.panelCheck(ev, /Some checks passed/.test(ev.panel.text), 'panel shows /Some checks passed/');
      h.panelCheck(ev, !/Verified/.test(ev.panel.text), 'badge never says Verified');
    },
  },
  {
    id: 4, name: 'A, B, C pages', timeout: 240,
    async run(h, ev) {
      const A = await h.openTab(h.site('/a'));
      const C = await h.openTab(h.site('/c'));
      await h.activate(A);
      const conv = await h.create('codex');
      const text = h.scenario('c4', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/b') }, save: 'openB' },
        { tool: 'chrome_use_click', args: { selector: '#act' }, save: 'actB' },
        { signal: 'acted' },
        { waitFor: 'c-activated', timeoutMs: 30000 },
        { tool: 'chrome_use_open', args: { url: h.site('/redirect') }, save: 'redirect' },
        { tool: 'chrome_use_open', args: { url: h.site('/popup-page') } },
        { tool: 'chrome_use_click', args: { selector: '#popup', follow: true }, save: 'popup' },
        { tool: 'chrome_use_read', args: {} },
        { say: 'Acted on B.' },
      ] });
      const { messageId } = await h.send(conv.id, text, { page: A });
      await h.waitFor(() => h.hasSignal(conv.id, 'acted'), { timeout: 60000, label: 'agent acted on B' });
      await h.activate(C);
      h.signal(conv.id, 'c-activated');
      const events = await h.turnEnd(conv.id, messageId);
      const t = turnOf(h, events, messageId);
      ev.tabs = { A: A.tabId, C: C.tabId };
      ev.actions = t.actions.map(brief);
      const user = h.one(events, 'user', d => d.messageId === messageId).data;
      ev.sentWith = user.page;
      h.assert.equal(user.page.tabId, A.tabId, 'sent with A');
      const open = t.actions.find(a => a.admitted?.tool === 'chrome_use_open' && a.target?.observed?.after?.url?.endsWith('/b'));
      h.assert.ok(open, 'the open of B is journaled');
      h.assert.equal(open.target.observed.after.provenance, 'actionReply', 'B comes from the action reply');
      const blob = JSON.stringify(events);
      h.assert.ok(!blob.includes(C.url) && !blob.includes(`chrome-tab:${C.tabId}`) && !events.some(e => e.data?.observed?.after?.chromeTabId === C.tabId || e.data?.observed?.before?.chromeTabId === C.tabId), 'no event mentions page C');
      const red = t.actions.find(a => a.actionId && a.admitted && JSON.stringify(a.target?.requested || '').includes('/redirect') || a.target?.observed?.after?.url?.includes('landed=1'));
      ev.redirect = red && brief(red);
      h.assert.ok(red?.target?.observed?.after?.url?.includes('landed=1'), 'the redirect lands on /b?landed=1');
      const pop = t.actions.find(a => a.target?.observed?.after?.url?.includes('popup=1'));
      ev.popup = pop && brief(pop);
      h.assert.ok(pop, 'the popup tab is listed as an acting page');
      ev.panel = await h.screenshot('check4', conv.id);
      h.panelCheck(ev, /Sent with[\s\S]*Page A/.test(ev.panel.transcript), 'card says Sent with Page A');
      h.panelCheck(ev, /Acted on[\s\S]*(Page B|127\.0\.0\.1)/.test(ev.panel.transcript), 'card says Acted on B');
      h.panelCheck(ev, /landed/i.test(ev.panel.transcript), 'card shows the redirect landing');
      h.panelCheck(ev, !/Page C/.test(ev.panel.transcript) && /Page C/.test(ev.panel.text), 'C only in the composer chip, not in the card');
      h.state.c4 = { conv, messageId };
    },
  },
  {
    id: 5, name: 'Stop timeout with process alive', timeout: 420,
    async run(h, ev) {
      const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
      // A fixture backend that ignores the interrupt and has a child that would outlive the leader.
      const stuck = async label => {
        const conv = await h.create('codex');
        const m = await h.send(conv.id, h.scenario(`c5${label}`, { ignoreInterrupt: true, steps: [{ tool: 'chrome_use_open', args: { url: h.site('/a') } }, { spawnChild: true }, { say: 'working', role: 'commentary' }, { sleep: 600000 }] }));
        await h.waitFor(() => h.hasSignal(conv.id, 'child-pid'), { timeout: 60000, label: 'child spawned' });
        const child = Number(fs.readFileSync(path.join(h.rig.accept, 'signals', `${conv.id}.child-pid`), 'utf8'));
        const stopAt = Date.now();
        await h.rig.service.call('conversationStop', { conversationId: conv.id });
        return { conv, messageId: m.messageId, child, stopAt };
      };
      if (process.env.CHECK5_SWITCH !== '1') {
        // Release build: the 10 s stop timeout kills and confirms the group.
        const { conv, messageId, child, stopAt } = await stuck('kill');
        const events = await h.turnEnd(conv.id, messageId, 40000);
        ev.stopToEndMs = Date.now() - stopAt;
        const exited = h.kinds(events, 'exited').at(-1);
        ev.exited = exited?.data; ev.processUnconfirmed = h.kinds(events, 'processUnconfirmed').length;
        ev.childAlive = alive(child);
        const summary = (await h.journal(conv.id)).conversation;
        ev.control = summary.control; ev.phase = summary.phase;
        ev.controlChanges = h.kinds(events, 'controlChanged').map(e => ({ sequence: e.sequence, from: e.data.from, to: e.data.to, reason: e.data.reason }));
        h.assert.equal(exited?.data.processExitConfirmed, true, 'the 10 s timeout killed and confirmed the group');
        h.assert.equal(ev.childAlive, false, 'the child that outlived the leader is gone');
        h.assert.equal(summary.control, 'stopped');
        h.assert.ok(ev.controlChanges.find(c => c.to === 'stopped')?.sequence > exited.sequence, 'control becomes stopped only after exited');
        return { status: 'pass', note: 'no-switch half (release build). The switch half runs with the e2e-tests build: results-check5-e2e.json.' };
      }
      // e2e-tests build with CHROME_USE_E2E_SKIP_STOP_KILL=1 in the service environment.
      const a = await stuck('switch');
      const ua = await h.waitEvent(a.conv.id, 'processUnconfirmed', () => true, 40000);
      ev.unconfirmed = ua.data;
      const sa = (await h.journal(a.conv.id)).conversation;
      ev.control = sa.control; ev.phase = sa.phase;
      h.assert.equal(sa.control, 'unknown');
      h.assert.ok(alive(a.child), 'the tree is still alive');
      const review = await h.rig.service.call('conversationRecoveryReview', { conversationId: a.conv.id });
      ev.reviewAlive = { process: review.process, actions: review.actions?.length, unresolved: review.unresolved };
      h.assert.equal(review.process.state, 'alive', 'review proves the group alive');
      const forkEarly = await h.rig.service.tryCall('conversationRecoveryFork', { conversationId: a.conv.id });
      ev.forkWhileAlive = forkEarly;
      h.assert.equal(forkEarly.ok, false, 'fork is refused while alive');
      const term = await h.rig.service.call('conversationRecoveryTerminate', { conversationId: a.conv.id });
      ev.terminate = term;
      h.assert.equal(term.confirmed, true, 'terminate confirms the whole group gone');
      h.assert.equal(alive(a.child), false, 'the child is gone');
      const after = await h.rig.service.call('conversationRecoveryReview', { conversationId: a.conv.id });
      ev.reviewAfter = after.process;
      h.assert.equal(after.process.state, 'exited');
      const fork = await h.rig.service.call('conversationRecoveryFork', { conversationId: a.conv.id });
      ev.fork = { forkedFrom: fork.forkedFrom, id: fork.conversation.id, queued: fork.conversation.queued, phase: fork.conversation.phase };
      const fe = await h.events(fork.conversation.id);
      ev.forkKinds = fe.map(e => e.kind);
      h.assert.equal(fork.forkedFrom, a.conv.id);
      h.assert.equal(fe[0]?.kind, 'recoveryContext', 'the fork starts with recoveryContext');
      h.assert.ok(!fe.some(e => ['user', 'turnStarted', 'ready'].includes(e.kind)), 'the fork sends nothing');
      h.assert.equal(fork.conversation.queued, 0);
      const orig = await h.events(a.conv.id);
      h.assert.ok(orig.some(e => e.kind === 'recoveryTerminated' && e.data.confirmed === true) && orig.some(e => e.kind === 'recoveryForked'), 'recoveryTerminated and recoveryForked journaled');
      // Restart the service with a second group still alive: review still proves it.
      const b = await stuck('restart');
      await h.waitEvent(b.conv.id, 'processUnconfirmed', () => true, 40000);
      const { stopService, startService } = await import('./rig.mjs');
      await stopService(h.rig);
      await startService(h.rig);
      h.assert.ok(alive(b.child), 'the group survived the service restart');
      const rb = await h.rig.service.call('conversationRecoveryReview', { conversationId: b.conv.id });
      ev.reviewAfterRestart = rb.process;
      h.assert.equal(rb.process.state, 'alive', 'after a restart the review still proves ownership');
      const tb = await h.rig.service.call('conversationRecoveryTerminate', { conversationId: b.conv.id });
      ev.terminateAfterRestart = tb;
      h.assert.equal(tb.confirmed, true);
      return { status: 'pass', note: 'e2e-tests build with CHROME_USE_E2E_SKIP_STOP_KILL=1. The Windows half (unknown after restart) needs the Windows runner and was not run here.' };
    },
  },
  {
    id: 6, name: 'Approval/stop interleave', timeout: 240,
    async run(h, ev) {
      // Part 1: approvals on. A slow action is admitted, a request is pending, then Stop.
      const conv = await h.create('codex', { fullAccess: false });
      const text = h.scenario('c6a', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/slow?ms=7000') }, async: true, save: 'slow' },
        { sleep: 800 },
        { request: { kind: 'command', tool: 'chrome_use_click' } },
        { tool: 'chrome_use_open', args: { url: h.site('/b?after-stop=1') } },
        { say: 'should not get here' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const request = await h.waitEvent(conv.id, 'request', d => d.messageId === messageId || true, 60000);
      await h.waitEvent(conv.id, 'actionAdmitted', d => d.messageId === messageId && d.tool === 'chrome_use_open');
      await h.rig.service.call('conversationStop', { conversationId: conv.id });
      const allow = await h.respond(conv.id, request, 'accept');
      ev.allowDuringStop = allow;
      h.assert.equal(allow.ok, false, 'Allow during stop is refused');
      const decline = await h.respond(conv.id, request, 'decline');
      ev.declineDuringStop = decline;
      h.assert.equal(decline.ok, true, 'decline during stop passes');
      const events = await h.turnEnd(conv.id, messageId);
      const t = turnOf(h, events, messageId);
      ev.turnEnded = t.end;
      ev.actions = t.actions.map(brief);
      h.assert.equal(t.end.outcome, 'interrupted');
      const stopSeq = h.kinds(events, 'controlChanged').find(e => e.data.to === 'stopRequested')?.sequence;
      ev.stopSequence = stopSeq;
      h.assert.ok(stopSeq, 'the stop is journaled');
      const lateAdmits = h.kinds(events, 'actionAdmitted').filter(e => e.sequence > stopSeq);
      h.assert.equal(lateAdmits.length, 0, 'no actionAdmitted after the stop');
      const slow = t.actions.find(a => a.admitted?.tool === 'chrome_use_open');
      h.assert.ok(slow.target, 'the slow action reported after the stop');
      h.assert.ok(slow.target.sequence > stopSeq, 'its report came after the stop');
      h.assert.equal(slow.target.afterStop, true);
      ev.slowPossiblyExecuted = slow.observed?.possiblyExecuted;
      h.assert.equal(h.siteLog().filter(r => r.query.includes('after-stop=1')).length, 0, 'the step after the approval never ran');
      // Part 2: full access, a request arriving during stop is declined, not approved.
      const conv2 = await h.create('codex', { fullAccess: true });
      const text2 = h.scenario('c6b', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/slow?ms=5000') }, async: true },
        { waitFor: 'never', timeoutMs: 60000 },
        { request: { kind: 'elicitation', tool: 'chrome_use_click' }, afterInterrupt: true },
      ] });
      const m2 = await h.send(conv2.id, text2);
      await h.waitEvent(conv2.id, 'actionAdmitted', d => d.messageId === m2.messageId);
      await h.rig.service.call('conversationStop', { conversationId: conv2.id });
      const ev2 = await h.turnEnd(conv2.id, m2.messageId);
      const replies = h.backendLog().filter(l => l.event === 'requestReply' && l.scenario === 'c6b');
      ev.fullAccessReplyDuringStop = replies.map(r => r.reply);
      h.assert.equal(replies.length, 1);
      h.assert.equal(replies[0].reply.allowed, false, 'full access does not auto-approve during stop');
      h.assert.equal(h.one(ev2, 'turnEnded', d => d.messageId === m2.messageId).data.outcome, 'interrupted');
    },
  },
  {
    id: 7, name: 'Same-name downloads', timeout: 180,
    async run(h, ev) {
      const conv = await h.create('codex');
      const text = h.scenario('c7', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/a') } },
        { parallel: [
          { tool: 'chrome_use_download_url', args: { url: h.site('/download/report.pdf?v=1'), path: '${cwd}/d1/report.pdf' }, save: 'd1' },
          { tool: 'chrome_use_download_url', args: { url: h.site('/download/report.pdf?v=2'), path: '${cwd}/d2/report.pdf' }, save: 'd2' },
        ] },
        { say: 'Saved both reports.' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const ready = h.kinds(events, 'artifactReady');
      // Downloads and screenshots are allocated in actionAdmitted.outputs; the
      // build journals artifactCreated only for recordings.
      const created = outputsOf(events, messageId);
      ev.created = created.map(e => e.data);
      ev.artifactCreatedEvents = h.kinds(events, 'artifactCreated').length;
      ev.ready = ready.map(e => e.data);
      ev.delivered = h.kinds(events, 'fileDelivered').map(e => e.data);
      const expected = new Set([h.sha256(pdfBody('1')), h.sha256(pdfBody('2'))]);
      h.assert.equal(created.length, 2, 'two artifacts');
      h.assert.equal(created.filter(e => e.data.name === 'report.pdf').length, 2, 'both named report.pdf');
      h.assert.equal(new Set(ready.map(e => e.data.digest)).size, 2, 'two digests');
      for (const r of ready) h.assert.ok(expected.has(r.data.digest), 'each blob matches its source');
      const t = turnOf(h, events, messageId);
      h.assert.equal(t.actions.filter(a => a.admitted?.tool === 'chrome_use_download_url' && a.observed?.verdict === 'confirmed').length, 2);
      // Overwrite the captured sources, then read both artifacts back.
      const out = path.join(h.folder(conv.id), 'workspace', '.chrome-use-out');
      const sources = fs.existsSync(out) ? fs.readdirSync(out).flatMap(d => fs.readdirSync(path.join(out, d)).map(f => path.join(out, d, f))) : [];
      ev.sourcesOverwritten = sources.length;
      for (const s of sources) { try { fs.chmodSync(s, 0o600); fs.writeFileSync(s, 'overwritten'); } catch (e) { ev.overwriteError = String(e); } }
      ev.reads = [];
      for (const c of created) {
        const r = await h.artifactRead(conv.id, c.data.artifactId);
        ev.reads.push({ artifactId: c.data.artifactId, digest: r.digest, computed: r.computed, bytes: r.bytes.length });
        h.assert.equal(r.computed, r.digest, 'read bytes match the recorded digest');
        h.assert.ok(expected.has(r.computed));
      }
      h.state.c7 = { conv, messageId, artifacts: created.map(e => e.data.artifactId), ready: ready.map(e => e.data) };
    },
  },
  {
    id: 8, name: 'Interrupted download', timeout: 150,
    async run(h, ev) {
      const conv = await h.create('codex');
      const text = h.scenario('c8', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/a') } },
        { tool: 'chrome_use_download_url', args: { url: h.site('/download/abort-report.pdf'), timeoutMs: 30000 }, save: 'dl' },
        { say: 'The download did not finish.' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const created = outputsOf(events, messageId);
      h.assert.equal(created.length, 1);
      const aid = created[0].data.artifactId;
      ev.artifact = artifactEvents(events, aid);
      const t = turnOf(h, events, messageId);
      ev.action = brief(t.actions.find(a => a.admitted?.tool === 'chrome_use_download_url'));
      h.assert.ok(!events.some(e => e.kind === 'artifactReady' && e.data.artifactId === aid), 'no Ready card');
      const un = events.find(e => e.kind === 'artifactUnavailable' && e.data.artifactId === aid);
      h.assert.equal(un?.data.reason, 'interrupted');
      h.assert.equal(ev.action.errorCode, 'download_interrupted');
      h.assert.equal(ev.action.verdict, 'failed');
      ev.panel = await h.screenshot('check8', conv.id);
      h.panelCheck(ev, /interrupted/i.test(ev.panel.transcript), 'result card says the download was interrupted');
    },
  },
  {
    id: 9, name: 'Deleted artifact file', timeout: 120,
    async run(h, ev) {
      const c7 = h.state.c7; h.assert.ok(c7, 'needs check 7');
      const aid = c7.artifacts[0];
      const digest = c7.ready.find(r => r.artifactId === aid).digest.replace('sha256:', '');
      const blobs = path.join(h.folder(c7.conv.id), 'artifacts');
      const blob = fs.readdirSync(blobs).find(f => f.startsWith(digest));
      h.assert.ok(blob, 'blob found');
      fs.chmodSync(blobs, 0o700);
      fs.rmSync(path.join(blobs, blob));
      ev.deleted = blob;
      const second = new Service(h.rig.serviceJson);
      ev.reads = [];
      for (const s of [h.rig.service, second, h.rig.service, second]) {
        const r = await s.tryCall('conversationArtifactRead', { conversationId: c7.conv.id, artifactId: aid, offset: 0, length: 1024 });
        ev.reads.push(r.ok ? 'ok' : r.error);
        h.assert.equal(r.ok, false, 'read reports unavailable');
      }
      const events = await h.events(c7.conv.id);
      const un = events.filter(e => e.kind === 'artifactUnavailable' && e.data.artifactId === aid && e.data.detectedBy === 'read');
      ev.unavailable = un.map(e => e.data);
      h.assert.equal(un.length, 1, 'exactly one artifactUnavailable {detectedBy: read}');
      // The other artifact still reads.
      const other = await h.artifactRead(c7.conv.id, c7.artifacts[1]);
      h.assert.equal(other.computed, other.digest);
    },
  },
  {
    id: 10, name: 'Cross-conversation artifact read', timeout: 60,
    async run(h, ev) {
      const c7 = h.state.c7; h.assert.ok(c7, 'needs check 7');
      const c1 = { conv: await h.create('codex') };
      const cross = await h.rig.service.tryCall('conversationArtifactRead', { conversationId: c1.conv.id, artifactId: c7.artifacts[1], offset: 0, length: 1024 });
      const unknown = await h.rig.service.tryCall('conversationArtifactRead', { conversationId: c1.conv.id, artifactId: randomUUID(), offset: 0, length: 1024 });
      const nonUuid = await h.rig.service.tryCall('conversationArtifactRead', { conversationId: c7.conv.id, artifactId: '../artifacts', offset: 0, length: 1024 });
      const nonUuid2 = await h.rig.service.tryCall('conversationArtifactRead', { conversationId: c7.conv.id, artifactId: 'not-a-uuid', offset: 0, length: 1024 });
      Object.assign(ev, { cross, unknown, nonUuid, nonUuid2 });
      for (const r of [cross, unknown, nonUuid, nonUuid2]) h.assert.equal(r.ok, false);
      h.assert.equal(cross.error, unknown.error, 'cross-conversation ids get the same error as unknown ids');
    },
  },
  {
    id: 11, name: 'Synthetic secrets in the journal and state', timeout: 240,
    async run(h, ev) {
      const r = randomUUID().slice(0, 8);
      const S = { pw: `SYNTH_PW_${r}x9`, cookie: `SYNTH_COOKIE_${r}abc`, tok: `SYNTH_TOK_${r}def`, bearer: `SYNTH_BEARER_${r}ghi` };
      const conv = await h.create('claude', { fullAccess: false });
      await fetch(h.site('/__secrets'), { method: 'POST', body: JSON.stringify({ id: r, cookie: S.cookie, tok: S.tok }) });
      const text = h.scenario('c11', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/login') } },
        { approve: { tool: 'chrome_use_fill', args: { selector: '#password', value: S.pw } } },
        { tool: 'chrome_use_click', args: { selector: '#submit' } },
        { tool: 'chrome_use_open', args: { url: h.site(`/cookie?id=${r}`) } },
        { tool: 'chrome_use_eval', args: { script: 'document.cookie' }, save: 'cookie' },
        { tool: 'chrome_use_click', args: { selector: '#tok' } },
        { tool: 'chrome_use_open', args: { url: h.site(`/b?access_token=${S.tok}&page=2`) } },
        { tool: 'chrome_use_network_route', args: { action: 'route', url: '**/never-called/**', setHeaders: { Authorization: `Bearer ${S.bearer}` } } },
        { say: `The cookie is sid=${S.cookie}.`, deltas: ['The cookie is sid=SYNTH_CO', `OKIE_${r}`, 'abc.'] },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const request = await h.waitEvent(conv.id, 'request', () => true, 60000);
      ev.requestShown = JSON.stringify(request.data).slice(0, 1500);
      const allow = await h.respond(conv.id, request, 'allow');
      ev.allow = allow;
      h.assert.equal(allow.ok, true, 'the approval is accepted');
      const events = await h.turnEnd(conv.id, messageId, 120000);
      ev.outcome = h.one(events, 'turnEnded', d => d.messageId === messageId).data.outcome;
      const reply = h.backendLog().find(l => l.event === 'canUseToolReply');
      ev.backendGotOriginal = reply?.updatedInputEqualsOriginal;
      h.assert.equal(reply?.updatedInputEqualsOriginal, true, 'the backend got the original (unredacted) input');
      const login = h.siteLog().find(l => l.path === '/api/login');
      ev.siteGotRealPassword = !!login && login.body.includes(S.pw);
      h.assert.ok(ev.siteGotRealPassword, 'the approved fill typed the real value');
      const folder = h.folder(conv.id);
      const files = [path.join(folder, 'events.jsonl'), path.join(folder, 'state.json'), path.join(h.rig.profileDir, 'conversations', 'index.json')];
      const read = JSON.stringify(await h.journal(conv.id));
      const hits = {};
      for (const f of files) { const t = fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : ''; hits[path.basename(f)] = (t.match(/SYNTH_[A-Z]+_[0-9a-f]+/g) || []); }
      hits.conversationRead = read.match(/SYNTH_[A-Z]+_[0-9a-f]+/g) || [];
      ev.hits = hits;
      ev.filesChecked = files.map(f => path.relative(h.rig.root, f));
      ev.masks = events.flatMap(e => (e.data?.redactions || []).map(x => ({ kind: e.kind, ...x }))).slice(0, 30);
      for (const [k, v] of Object.entries(hits)) h.assert.equal(v.length, 0, `${k} holds no SYNTH_ value`);
      h.assert.ok(fs.existsSync(files[0]) && fs.existsSync(files[1]), 'journal and state exist');
    },
  },
  {
    id: 12, name: 'Stalled progress', timeout: 420,
    async run(h, ev) {
      // Long parts first, in parallel: a 200 s silent turn and a 60 s watch.
      const silent = await h.create('codex');
      const sm = await h.send(silent.id, h.scenario('c12silent', { steps: [{ say: 'thinking', role: 'commentary' }, { sleep: 200000 }, { say: 'done' }] }));
      const watch = await h.create('codex');
      const wm = await h.send(watch.id, h.scenario('c12watch', { steps: [{ tool: 'chrome_use_open', args: { url: h.site('/a') } }, { tool: 'chrome_use_diag_pages', args: { watchSeconds: 60, timeoutMs: 90000 } }, { say: 'watched' }] }));
      const conv = await h.create('codex');
      const steps = [{ tool: 'chrome_use_open', args: { url: h.site('/static') } }];
      for (let i = 0; i < 4; i++) steps.push({ tool: 'chrome_use_read', args: {} });
      steps.push({ signal: 'paging' }, { tool: 'chrome_use_open', args: { url: h.site('/list?page=1') } }, { tool: 'chrome_use_read', args: {} });
      for (let i = 2; i <= 6; i++) steps.push({ tool: 'chrome_use_click', args: { selector: '#next' } }, { tool: 'chrome_use_read', args: {} });
      steps.push({ say: 'Read six pages.' });
      const { messageId } = await h.send(conv.id, h.scenario('c12', { steps }));
      const events = await h.turnEnd(conv.id, messageId, 180000);
      const stalls = events.filter(e => e.kind === 'progressStalled');
      ev.stalls = stalls.map(e => ({ sequence: e.sequence, ...e.data }));
      ev.resumed = h.kinds(events, 'progressResumed').map(e => e.data);
      const listOpen = h.actions(events, messageId).find(a => a.admitted?.tool === 'chrome_use_open' && a.target?.observed?.after?.url?.includes('/list'));
      ev.readDigests = h.actions(events, messageId).filter(a => a.admitted?.tool === 'chrome_use_read').map(a => a.target?.resultDigest);
      const failures = [];
      const want = (ok, label) => { if (!ok) failures.push(label); };
      want(stalls.length >= 1 && stalls[0].data.kind === 'repeatedIdenticalResult', 'four identical reads raise one progressStalled (repeatedIdenticalResult)');
      want(stalls.filter(x => x.data.kind === 'repeatedIdenticalResult').length <= 1, 'one stall per episode');
      want(stalls.every(x => x.sequence < listOpen.admitted.sequence), 'reading six pages through next raises none');
      ev.summary = h.one(events, 'turnSummary', d => d.messageId === messageId)?.data;
      const we = await h.turnEnd(watch.id, wm.messageId, 150000);
      ev.watchStalls = we.filter(e => e.kind === 'progressStalled').map(e => e.data);
      want(ev.watchStalls.length === 0, 'a 60 s diag pages watch raises none');
      const se = await h.turnEnd(silent.id, sm.messageId, 260000);
      ev.silentStalls = se.filter(e => e.kind === 'progressStalled').map(e => e.data);
      ev.silentResumed = se.filter(e => e.kind === 'progressResumed').map(e => e.data);
      want(ev.silentStalls.filter(x => x.kind === 'noActivity').length === 1, 'a 200 s silent turn raises noActivity');
      ev.failures = failures;
      const timings = h.rig.proxy.timings.filter(t => t.op === 'agentActionBegin').map(t => t.ms).sort((a, b) => a - b);
      ev.leaseLatencyMs = { n: timings.length, p50: timings[Math.floor(timings.length / 2)], p95: timings[Math.floor(timings.length * 0.95)], max: timings.at(-1), note: 'measured at the driver proxy (round trip through one extra loopback hop)' };
      if (failures.length) throw new Error(failures.join('; '));
    },
  },
  {
    id: 13, name: 'Refresh resume consistency', timeout: 200,
    async run(h, ev) {
      const { PanelTimeline } = await import(path.join(h.rig.extDir, 'panel-state.js'));
      const conv = await h.create('codex');
      const steps = [];
      for (let i = 1; i <= 4; i++) steps.push({ tool: 'chrome_use_open', args: { url: h.site(`/list?page=${i}`) } }, { say: `page ${i}`, role: 'commentary' }, { sleep: 1500 });
      steps.push({ say: 'done' });
      const { messageId } = await h.send(conv.id, h.scenario('c13', { steps }));
      // Incremental reader, as an open panel polls.
      const inc = new PanelTimeline(); let cursor = 0; let polls = 0;
      const poll = async () => { const j = await h.rig.service.call('conversationRead', { conversationId: conv.id, after: cursor }); inc.apply(j.journal.events, j.journal.cursor); cursor = j.journal.cursor; polls++; };
      const panel1 = h.panel ||= await h.openPanel();
      await h.rig.ev(`chrome.storage.local.set({ ab_panel_conversation: ${JSON.stringify(conv.id)}, ab_panel_conversation_scope: "all" })`);
      const win2 = await h.rig.ev('chrome.windows.create({ url: "about:blank" })');
      const panel2 = await h.openPanel(win2.id);
      await h.rig.browser.send('Page.reload', {}, panel1.sid);
      while (!h.one(await h.events(conv.id), 'turnEnded', d => d.messageId === messageId)) {
        await poll();
        if (polls === 3) await h.rig.browser.send('Page.reload', {}, panel1.sid); // close and reopen mid-turn
        await h.sleep(700);
      }
      await h.turnEnd(conv.id, messageId);
      await poll(); await poll();
      const fresh = new PanelTimeline();
      const all = await h.events(conv.id);
      fresh.apply(all, all.at(-1).sequence);
      const a = JSON.stringify(plain({ turns: fresh.turns, order: fresh.turnOrder, rows: fresh.rows }));
      const b = JSON.stringify(plain({ turns: inc.turns, order: inc.turnOrder, rows: inc.rows }));
      ev.polls = polls; ev.events = all.length;
      h.assert.equal(b, a, 'incremental projection deep-equals a fresh read from 0');
      await h.sleep(3000);
      const count = sid => h.rig.cdp.evaluate(sid, `(() => { const t = (document.getElementById('transcript') || document.body).innerText; return { sends: (t.match(/run scenario c13/g) || []).length, text: t.slice(0, 300) }; })()`);
      ev.panel1 = await count(panel1.sid); ev.panel2 = await count(panel2.sid);
      h.panelCheck(ev, ev.panel1.sends === 1, 'panel 1 (reopened mid-turn): one card for the message');
      h.panelCheck(ev, ev.panel2.sends === 1, 'panel 2: one card for the message');
      const shot = await h.rig.browser.send('Page.captureScreenshot', { format: 'png' }, panel2.sid);
      fs.writeFileSync(path.join(h.out, 'panel-check13-second.png'), Buffer.from(shot.data, 'base64'));
      await h.rig.ev(`chrome.windows.remove(${win2.id})`).catch(() => {});
    },
  },
  {
    id: 14, name: 'Replay producing one turn', timeout: 120,
    async run(h, ev) {
      const conv = await h.create('codex');
      const text = h.scenario('c14', { steps: [{ tool: 'chrome_use_open', args: { url: h.site('/a') } }, { say: 'once' }] });
      const messageId = randomUUID();
      h.rig.proxy.dropReply(messageId);
      const first = await h.rig.service.tryCall('conversationSend', { conversationId: conv.id, messageId, text });
      ev.firstReply = first;
      h.assert.equal(first.ok, false, 'the first reply was dropped');
      const retry = await h.rig.service.tryCall('conversationSend', { conversationId: conv.id, messageId, text });
      ev.retry = retry;
      h.assert.equal(retry.ok, true, 'the retry is answered');
      const events = await h.turnEnd(conv.id, messageId);
      h.assert.equal(events.filter(e => e.kind === 'user' && e.data.messageId === messageId).length, 1, 'one user event');
      h.assert.equal(events.filter(e => e.kind === 'turnStarted' && e.data.messageId === messageId).length, 1, 'one turnStarted');
      ev.panel = await h.screenshot('check14', conv.id);
      h.panelCheck(ev, (ev.panel.transcript.match(/run scenario c14/g) || []).length === 1, 'one user message card');
      return { status: 'pass', note: 'The reply was dropped by the driver proxy on the service socket after the service stored the message; the extension bridge itself was not fault-injected.' };
    },
  },
  {
    id: 16, name: 'Stealth', timeout: 300,
    async run(h, ev) {
      const site = h.site('/stealth');
      // The same browser steps for the panel run and the comparison runs.
      const browserSteps = [
        { tool: 'chrome_use_open', args: { url: site } },
        { tool: 'chrome_use_read', args: {} },
        { tool: 'chrome_use_click', args: { selector: '#act', observe: true } },
        { tool: 'chrome_use_open', args: { url: h.site('/b') } },
        { tool: 'chrome_use_click', args: { selector: '#act' } },
        { tool: 'chrome_use_open', args: { url: site } },
      ];
      const enablesOf = methods => [...new Set(methods.filter(m => /\.enable$/.test(m) || m.startsWith('Target.setAutoAttach') || m.startsWith('Emulation.setFocusEmulation')))].sort();
      const delta = async fn => { const before = (await h.cdpLog()).log.length; await fn(); const { log, restarted } = await h.cdpLog(); h.assert.ok(!restarted, 'the extension worker kept the CDP log'); return log.slice(before).map(x => x.method); };
      // 1. The panel run (scenarios 1 and 4 shapes: read, act with observe, navigate, act).
      let events; let messageId;
      const panelMethods = await delta(async () => {
        const conv = await h.create('codex');
        const steps = [...browserSteps.slice(0, 3), { sleep: 3500 },
          { tool: 'chrome_use_expect', args: { kind: 'request', filter: '/log/stealth', noWait: true } },
          ...browserSteps.slice(3), { sleep: 2500 }, { say: 'done' }];
        ({ messageId } = await h.send(conv.id, h.scenario('c16', { steps })));
        events = await h.turnEnd(conv.id, messageId);
      });
      const counts = {}; for (const m of panelMethods) counts[m] = (counts[m] || 0) + 1;
      ev.methodCounts = counts;
      ev.panelEnables = enablesOf(panelMethods);
      h.assert.ok(panelMethods.length > 0, 'the CDP log captured the relay traffic');
      h.assert.ok(!counts['Runtime.enable'], 'no Runtime.enable');
      h.assert.ok(!Object.keys(counts).some(m => m.startsWith('Console.') || m === 'Log.enable'), 'no Console.* or Log.enable');
      // 2. The same steps through the v1.5.214 binary's MCP (no panel), same relay.
      if (process.env.OLD_BIN) {
        const oldMethods = await delta(async () => {
          const mcp = await h.plainMcp(process.env.OLD_BIN, { HOME: h.rig.home, AGENT_BROWSER_BROWSER: h.rig.profile, AGENT_BROWSER_SESSION: 'old-stealth', CHROME_USE_NO_UPDATE_CHECK: '1' });
          const env = { ...process.env, HOME: h.rig.home, AGENT_BROWSER_BROWSER: h.rig.profile, AGENT_BROWSER_SESSION: 'old-stealth', CHROME_USE_NO_UPDATE_CHECK: '1' };
          const { spawnSync } = await import('node:child_process');
          try {
            for (const s of browserSteps) {
              // v1.5.214's MCP has no `observe` argument; its CLI has --observe.
              if (s.args.observe) { const r = spawnSync(process.env.OLD_BIN, ['click', s.args.selector, '--observe', '--json'], { env, encoding: 'utf8', timeout: 60000 }); ev.oldResults = [...(ev.oldResults || []), r.status === 0]; ev.oldObserve = (r.stdout || r.stderr || '').slice(0, 300); continue; }
              ev.oldResults = [...(ev.oldResults || []), !(await mcp.call(s.tool, s.args))?.isError];
            }
          } finally { mcp.close(); }
        });
        ev.v15214Enables = enablesOf(oldMethods);
        ev.beyondV15214 = ev.panelEnables.filter(m => !ev.v15214Enables.includes(m));
        h.assert.ok(ev.oldResults.every(Boolean), 'the v1.5.214 comparison run succeeded');
        h.assert.deepEqual(ev.beyondV15214, [], 'no domain enable beyond what v1.5.214 sends for the same steps');
      }
      ev.beyondPlanList = ev.panelEnables.filter(m => !['Page.enable', 'Network.enable', 'Target.setAutoAttach', 'Emulation.setFocusEmulationEnabled'].includes(m));
      // 3. DOM: the MutationObserver page reports what changed.
      const beacons = h.siteLog().filter(l => l.path === '/log/stealth').map(l => { try { return JSON.parse(l.body); } catch { return null; } }).filter(Boolean);
      ev.domReports = beacons.length;
      ev.mutations = [...new Set(beacons.flatMap(b => b.mutations.map(m => JSON.stringify(m))))].slice(0, 20);
      ev.rootAttrs = [...new Set(beacons.flatMap(b => b.rootAttrs))];
      h.assert.ok(beacons.length > 0, 'the DOM probe reported');
      h.assert.equal(ev.mutations.length, 0, 'no added attributes or nodes on the page');
      h.assert.equal(ev.rootAttrs.length, 0, 'no attributes on <html>');
      h.assert.equal(events.filter(e => e.kind === 'artifactCreated' && e.data.kind === 'recording').length, 0, 'no recording without the message flag');
      // 4. The request log after a panel observe holds no more than 1000
      // entries (plan D3 correction): 1500 requests after an observed click.
      const burst = await h.create('codex');
      const bm = await h.send(burst.id, h.scenario('c16burst', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/burst') } },
        { tool: 'chrome_use_click', args: { selector: '#act', observe: true } },
        { tool: 'chrome_use_click', args: { selector: '#go' } },
        { tool: 'chrome_use_wait', args: { text: '1500', timeoutMs: 60000 } },
        { sleep: 1000 },
        { tool: 'chrome_use_expect', args: { kind: 'request', filter: '/ping', noWait: true } },
        { say: 'done' },
      ] }));
      await h.turnEnd(burst.id, bm.messageId, 120000);
      const probe = h.backendLog().filter(l => l.event === 'tool' && l.scenario === 'c16burst' && l.tool === 'chrome_use_expect').at(-1);
      ev.pingsServed = h.siteLog().filter(l => l.path === '/ping').length;
      ev.requestLogMatches = Number(/actual\\?"?: ?(\d+)/.exec(probe?.result || '')?.[1]);
      h.assert.ok(ev.pingsServed >= 1500, 'the page sent 1500 requests');
      h.assert.ok(ev.requestLogMatches > 0 && ev.requestLogMatches <= 1000, 'the request log holds no more than 1000 entries');
    },
  },
  {
    id: 17, name: 'Recording', timeout: 360,
    async run(h, ev) {
      const recSteps = (stop = true) => [
        { tool: 'chrome_use_open', args: { url: h.site('/a') } },
        { tool: 'chrome_use_record', args: { action: 'start' }, save: 'rec' },
        { tool: 'chrome_use_open', args: { url: h.site('/b') } },
        { tool: 'chrome_use_click', args: { selector: '#act' } },
        { sleep: 1500 },
        ...(stop ? [{ tool: 'chrome_use_record', args: { action: 'stop', artifactId: '${rec.artifactId}' } }] : []),
        { say: 'recorded' },
      ];
      const waitRec = async (convId, messageId, timeout = 90000) => h.waitFor(async () => {
        const events = await h.events(convId);
        const c = events.find(e => e.kind === 'artifactCreated' && e.data.kind === 'recording' && e.data.messageId === messageId);
        if (!c) return null;
        const done = events.find(e => (e.kind === 'artifactReady' || e.kind === 'artifactUnavailable') && e.data.artifactId === c.data.artifactId);
        return done ? { events, created: c, done } : null;
      }, { timeout, label: 'recording outcome' });
      // With the flag.
      const conv = await h.create('codex');
      const m = await h.send(conv.id, h.scenario('c17', { steps: recSteps() }), { record: true });
      const r = await waitRec(conv.id, m.messageId);
      ev.recorded = artifactEvents(r.events, r.created.data.artifactId);
      h.assert.deepEqual(ev.recorded.map(e => e.kind), ['artifactCreated', 'artifactState', 'artifactReady']);
      h.assert.equal(ev.recorded[0].state, 'capturing'); h.assert.equal(ev.recorded[1].state, 'finalizing');
      h.assert.equal(r.done.data.mime, 'video/webm');
      // Without the flag.
      const conv2 = await h.create('codex');
      const m2 = await h.send(conv2.id, h.scenario('c17noflag', { steps: recSteps() }));
      const e2 = await h.turnEnd(conv2.id, m2.messageId);
      const refused = h.backendLog().find(l => l.event === 'tool' && l.scenario === 'c17noflag' && l.tool === 'chrome_use_record');
      ev.noFlagStart = refused && { isError: refused.isError, result: refused.result.slice(0, 400) };
      h.assert.equal(refused?.isError, true, 'chrome_use_record start is refused without the flag');
      h.assert.equal(e2.filter(e => e.kind === 'artifactCreated' && e.data.kind === 'recording').length, 0);
      // ffmpeg missing, then an ffmpeg that cannot encode VP8 (fresh conversations: fresh daemons).
      const fakeDir = path.join(h.rig.work, 'fake-ffmpeg'); fs.mkdirSync(fakeDir, { recursive: true });
      fs.writeFileSync(path.join(fakeDir, 'ffmpeg'), '#!/bin/sh\necho "Unknown encoder \'libvpx\'" >&2\nexit 1\n', { mode: 0o755 });
      for (const [label, PATH, reason] of [['missing', '/usr/bin:/bin:/usr/sbin:/sbin', 'encoderMissing'], ['nolibvpx', `${fakeDir}:/usr/bin:/bin`, 'encoderFailed']]) {
        const c = await h.create('codex');
        const mm = await h.send(c.id, h.scenario(`c17${label}`, { steps: [{ mcpStart: { PATH } }, ...recSteps()] }), { record: true });
        const rr = await waitRec(c.id, mm.messageId).catch(e => ({ error: String(e) }));
        ev[label] = rr.error ? rr : artifactEvents(rr.events, rr.created.data.artifactId);
        const recTool = h.backendLog().filter(l => l.event === 'tool' && l.scenario === `c17${label}` && l.tool === 'chrome_use_record').map(l => ({ isError: l.isError, result: l.result.slice(0, 300) }));
        ev[label + 'Tools'] = recTool;
        h.assert.equal(rr.done?.data.reason, reason, `${label}: ${reason}`);
      }
      // Two consecutive recorded turns; the first is stopped by the host after its turn.
      const conv3 = await h.create('codex');
      const t1 = await h.send(conv3.id, h.scenario('c17first', { steps: recSteps(false) }), { record: true });
      await h.turnEnd(conv3.id, t1.messageId);
      const t2 = await h.send(conv3.id, h.scenario('c17second', { steps: recSteps() }), { record: true });
      const r1 = await waitRec(conv3.id, t1.messageId);
      const r2 = await waitRec(conv3.id, t2.messageId, 120000);
      const all = r2.events;
      ev.first = artifactEvents(all, r1.created.data.artifactId);
      ev.second = artifactEvents(all, r2.created.data.artifactId);
      h.assert.equal(r1.done.kind, 'artifactReady', 'first recording ready');
      h.assert.equal(r2.done.kind, 'artifactReady', 'second recording ready');
      const firstFinalizing = ev.first.find(e => e.state === 'finalizing')?.sequence;
      const firstReady = ev.first.find(e => e.kind === 'artifactReady')?.sequence;
      const secondStart = h.one(all, 'turnStarted', d => d.messageId === t2.messageId)?.sequence;
      ev.interleave = { firstFinalizing, firstReady, secondTurnStarted: secondStart, secondCreated: ev.second[0]?.sequence, startedWhileFinalizing: secondStart > firstFinalizing && secondStart < firstReady };
    },
  },
  {
    id: 18, name: 'Turn attribution under load', timeout: 240,
    async run(h, ev) {
      const conv = await h.create('codex');
      const a = await h.send(conv.id, h.scenario('c18a', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/a') } },
        { tool: 'chrome_use_download_url', args: { url: h.site('/download/slow-report.pdf?ms=9000&v=7'), timeoutMs: 60000 }, async: true, save: 'slow' },
        { waitFor: 'never', timeoutMs: 120000 },
        { say: 'A finished' },
      ] }));
      const admitted = await h.waitEvent(conv.id, 'actionAdmitted', d => d.messageId === a.messageId && d.tool === 'chrome_use_download_url');
      await h.sleep(1500);
      const bText = h.scenario('c18b', { steps: [{ waitFor: 'go', timeoutMs: 120000 }, { assessFile: 'c18b-assess' }, { say: 'B done' }] });
      const b = await h.send(conv.id, bText, { direct: true });
      const aEvents = await h.turnEnd(conv.id, a.messageId, 90000);
      const artifactId = admitted.data.outputs?.[0]?.artifactId;
      ev.aArtifact = artifactId;
      await h.waitFor(async () => (await h.events(conv.id)).some(e => (e.kind === 'artifactReady' || e.kind === 'artifactUnavailable') && e.data.artifactId === artifactId), { timeout: 60000, label: 'A artifact outcome' });
      fs.writeFileSync(path.join(h.rig.accept, 'scenarios', 'c18b-assess.json'), JSON.stringify({ status: 'complete', requirements: [{ text: 'Download the report', status: 'met', check: { kind: 'downloadType', predicate: 'application/pdf', evidence: artifactId } }] }));
      h.signal(conv.id, 'go');
      const events = await h.turnEnd(conv.id, b.messageId, 90000);
      const created = outputsOf(events, a.messageId).find(o => o.data.artifactId === artifactId)?.data;
      ev.artifactEvents = artifactEvents(events, artifactId);
      ev.aOutcome = h.one(events, 'turnEnded', d => d.messageId === a.messageId).data.outcome;
      ev.bFinal = h.finalAssessment(events, b.messageId);
      h.assert.equal(created?.messageId, a.messageId, 'the download artifact lands under A');
      const slow = h.actions(events, a.messageId).find(x => x.admitted?.tool === 'chrome_use_download_url');
      ev.slow = brief(slow);
      h.assert.ok(ev.artifactEvents.some(e => e.kind === 'artifactReady'), 'A download finished and is Ready');
      h.assert.notEqual(ev.bFinal.requirements[0].computed, 'checked', "B's assessment cannot cite A's artifact");
      const bEvents = events.filter(e => e.data?.messageId === b.messageId);
      h.assert.ok(!JSON.stringify(bEvents).includes(slow.actionId) || bEvents.every(e => e.kind === 'taskAssessment'), "B's events carry nothing from A");
      h.assert.ok(!bEvents.some(e => ['actionAdmitted', 'browserActionTarget', 'actionObserved', 'artifactCreated'].includes(e.kind) && (e.data.actionId === slow.actionId || e.data.artifactId === artifactId)));
      ev.panel = await h.screenshot('check18', conv.id);
    },
  },
  {
    id: 19, name: 'Assessment counterexamples live', timeout: 150,
    async run(h, ev) {
      const conv = await h.create('codex');
      const text = h.scenario('c19', { steps: [
        { tool: 'chrome_use_open', args: { url: h.site('/a') }, save: 'o' },
        { tool: 'chrome_use_download_url', args: { url: h.site('/download/empty.pdf') }, save: 'z' },
        { tool: 'chrome_use_screenshot', args: {}, save: 's' },
        { assess: { status: 'complete', reason: 'paid', requirements: [
          { text: 'Pay the invoice', status: 'met', check: { kind: 'expect', predicate: 'text body contains Paid', evidence: '${o.actionId}' } },
          { text: 'Receipt saved', status: 'met', check: { kind: 'downloadType', predicate: 'application/pdf', evidence: '${z.artifactId}' } },
          { text: 'Confirmation shown', status: 'met', check: { kind: 'downloadType', predicate: 'image/png', evidence: '${s.artifactId}' } },
        ] } },
        { say: 'Paid.' },
      ] });
      const { messageId } = await h.send(conv.id, text);
      const events = await h.turnEnd(conv.id, messageId);
      const final = h.finalAssessment(events, messageId);
      ev.assessment = final;
      h.assert.equal(final.status, 'notChecked');
      for (const r of final.requirements) { h.assert.notEqual(r.computed, 'checked'); h.assert.ok(r.because, 'each requirement says why'); }
      ev.panel = await h.screenshot('check19', conv.id);
      h.panelCheck(ev, /Not checked/.test(ev.panel.text), 'panel shows /Not checked/');
    },
  },
  {
    id: 20, name: 'Late Begin across Stop', timeout: 900,
    async run(h, ev) {
      ev.runs = [];
      const agents = (process.env.LATE_AGENTS || 'codex,claude,pi,opencode').split(',');
      for (const agent of agents) for (const mode of ['stop', 'stopAndSend']) {
        const run = async () => {
          const r = { agent, mode };
          ev.runs.push(r);
          const tag = `${agent}-${mode}-${randomUUID().slice(0, 6)}`;
          const conv = await h.create(agent);
          const gate = h.rig.proxy.holdBegin(conv.id, 1);
          const a = await h.send(conv.id, h.scenario(`c20a-${tag}`, { steps: [{ say: 'working', role: 'commentary' }, { tool: 'chrome_use_open', args: { url: h.site(`/a?late=${tag}`) } }, { say: 'A done' }] }));
          await h.waitFor(() => gate.held().length > 0, { timeout: 60000, label: 'A Begin held' });
          const held = gate.held()[0];
          r.heldAction = held.request.params.actionId; r.heldGeneration = held.request.params.generation;
          const bText = h.scenario(`c20b-${tag}`, { steps: [{ tool: 'chrome_use_open', args: { url: h.site(`/b?late=${tag}-B`) } }, { say: 'B done' }] });
          let b;
          if (mode === 'stop') {
            await h.rig.service.call('conversationStop', { conversationId: conv.id });
            await h.turnEnd(conv.id, a.messageId, 60000);
            b = await h.send(conv.id, bText);
            // The queue pauses after a turn that did not complete; the user resumes it.
            r.resume = await h.rig.service.tryCall('conversationResumeQueue', { conversationId: conv.id });
          } else {
            b = await h.send(conv.id, bText, { direct: true });
          }
          const bStarted = await h.waitEvent(conv.id, 'turnStarted', d => d.messageId === b.messageId, 60000);
          // B's process is ready (a new generation) before A's Begin is released.
          const ready = await h.waitFor(async () => {
            const all = await h.events(conv.id);
            const aEnded = h.one(all, 'turnEnded', d => d.messageId === a.messageId);
            return aEnded && all.find(e => e.kind === 'ready' && e.sequence > aEnded.sequence);
          }, { timeout: 60000, label: 'B ready' }).catch(() => null);
          h.rig.proxy.releaseBegin(conv.id);
          await h.waitFor(() => gate.replies.length > 0, { timeout: 30000, label: 'released Begin answered' });
          r.releasedReply = gate.replies[0].reply;
          const events = await h.turnEnd(conv.id, b.messageId, 90000);
          const aEnd = h.one(events, 'turnEnded', d => d.messageId === a.messageId);
          r.aOutcome = aEnd.data.outcome;
          r.aGeneration = aEnd.data.generation;
          r.bGeneration = bStarted.data.generation;
          r.restartMs = ready ? ready.at - aEnd.at : null;
          r.retired = h.kinds(events, 'generationRetired').map(e => e.data);
          r.aActions = h.actions(events, a.messageId).map(brief);
          r.bActions = h.actions(events, b.messageId).map(brief);
          r.siteHitsA = h.siteLog().filter(l => l.query.includes(`late=${tag}`) && !l.query.includes(`late=${tag}-B`)).length;
          r.siteHitsB = h.siteLog().filter(l => l.query.includes(`late=${tag}-B`)).length;
          const reply = r.releasedReply;
          h.assert.ok(reply.error || reply.result?.admitted === false, `${agent}/${mode}: A's Begin is refused`);
          h.assert.ok(r.bGeneration > r.heldGeneration, `${agent}/${mode}: B runs on a new generation`);
          h.assert.equal(r.siteHitsA, 0, `${agent}/${mode}: the site saw no request from A's tool`);
          h.assert.ok(!events.some(e => e.data?.actionId === r.heldAction && e.kind === 'actionAdmitted'), `${agent}/${mode}: A's held action was never admitted`);
          h.assert.ok(r.bActions.every(x => x.actionId !== r.heldAction), `${agent}/${mode}: B shows nothing from A`);
          h.assert.ok(r.siteHitsB >= 1, `${agent}/${mode}: B ran`);
        };
        try { await run(); ev.runs.at(-1).pass = true; } catch (e) { ev.runs.at(-1).pass = false; ev.runs.at(-1).failure = String(e.message || e).slice(0, 400); }
      }
      const failedRuns = ev.runs.filter(r => !r.pass);
      if (failedRuns.length) throw new Error(`late-Begin runs failing: ${failedRuns.map(r => `${r.agent}/${r.mode}: ${r.failure}`).join(' | ')}`);
      const restarts = ev.runs.map(r => r.restartMs).filter(x => x != null);
      ev.restartMs = { max: Math.max(...restarts), all: ev.runs.map(r => `${r.agent}/${r.mode}: ${r.restartMs}`) };
      if (ev.runs.some(r => r.restartMs == null || r.restartMs >= 5000)) return { status: 'fail', note: 'restart cost from turnEnded to B ready is not under 5 s for every backend' };
    },
  },
  {
    id: 'R', name: 'Real backends: scenarios 1, 4 and 12', timeout: 1500,
    async run(h, ev) {
      ev.runs = [];
      const backends = [];
      if (process.env.REAL_CODEX !== '0') backends.push({ agent: 'codex' });
      if (process.env.REAL_OPENCODE_BIN) backends.push({ agent: 'opencode', executable: process.env.REAL_OPENCODE_BIN, model: process.env.REAL_OPENCODE_MODEL });
      const prompts = {
        s1: () => ({ text: `In your browser session open ${h.site('/form')}, fill the Name field (#name) with Alice, then click the element with selector #does-not-exist (it does not exist; that failure is expected), then click the "Save draft" button (#save-ok) and confirm the status text says "Draft saved". Keep the answer to one sentence.` }),
        s4: (A) => ({ text: `This is my tab. Open ${h.site('/b')} in your own browser session, click the Act button (#act), then open ${h.site('/redirect')} and tell me in one sentence which URL it landed on.`, page: A }),
        s12: () => ({ text: `Use chrome_use_read on ${h.site('/static')}: open it, then call chrome_use_read four times in a row with no arguments and nothing in between. Then open ${h.site('/list?page=1')} and read each page through page 6 by clicking the "next" link (#next) and reading after each click. Answer in one sentence.` }),
      };
      for (const b of backends) {
        for (const s of ['s1', 's4', 's12']) for (let attempt = 1; attempt <= 2; attempt++) {
          const r = { agent: b.agent, scenario: s, attempt };
          ev.runs.push(r);
          try {
            const conv = await h.create(b.agent, { real: true, executable: b.executable, model: b.model });
            let A, C;
            if (s === 's4') { A = await h.openTab(h.site('/a')); C = await h.openTab(h.site('/c')); await h.activate(A); }
            const p = prompts[s](A);
            const m = await h.send(conv.id, p.text, { page: p.page });
            if (s === 's4') {
              await h.waitEvent(conv.id, 'browserActionTarget', d => d.messageId === m.messageId, 180000).catch(() => null);
              await h.activate(C);
            }
            const events = await h.turnEnd(conv.id, m.messageId, 400000);
            const t = turnOf(h, events, m.messageId);
            const tools = events.filter(e => e.kind === 'tool' && e.data.completed && e.data.messageId === m.messageId);
            r.outcome = t.end.outcome; r.error = t.end.error;
            r.ready = h.kinds(events, 'ready').map(e => ({ backendVersion: e.data.backendVersion, model: e.data.model }));
            r.actions = t.actions.map(brief);
            r.toolSteps = tools.map(e => ({ name: e.data.item?.tool || e.data.item?.name || e.data.item?.title, actionIds: e.data.actionIds, codemode: e.data.codemode }));
            r.linked = r.actions.filter(a => tools.some(e => e.data.actionIds?.includes(a.actionId))).length;
            r.assessment = t.final && { source: t.final.source, status: t.final.status, declaredStatus: t.final.declaredStatus, requirements: t.final.requirements?.map(q => ({ text: q.text, declared: q.declared, computed: q.computed, check: q.check?.kind })) };
            r.stalls = t.stalls.map(x => x.kind);
            r.answer = events.filter(e => e.kind === 'textFinal' && e.data.messageId === m.messageId).map(e => e.data.text).join(' ').slice(0, 400);
            r.summary = h.one(events, 'turnSummary', d => d.messageId === m.messageId)?.data;
            if (s === 's4') {
              const user = h.one(events, 'user', d => d.messageId === m.messageId).data;
              r.sentWithA = user.page?.tabId === A.tabId;
              r.mentionsC = JSON.stringify(events.filter(e => e.data?.messageId === m.messageId)).includes(C.url);
              r.actingPages = r.actions.filter(a => a.after?.provenance === 'actionReply').map(a => a.after.url);
            }
            r.pass = r.outcome === 'completed' && r.actions.length > 0 && r.actions.every(a => a.verdict || a.outcome) && (s !== 's4' || (r.sentWithA && !r.mentionsC));
          } catch (e) { r.error = String(e.message || e).slice(0, 500); r.pass = false; }
          // One retry, only when the provider refused the model before any tool ran.
          if (r.pass || !/deprecated|no longer available|set_config_option/i.test(r.error || '')) break;
          r.retried = true;
        }
      }
      if (!backends.length) return { status: 'skip', note: 'no real backend configured' };
      const failed = ev.runs.filter(r => !r.pass && !r.retried);
      if (failed.length) return { status: 'fail', note: `real runs not passing: ${failed.map(r => `${r.agent}/${r.scenario}`).join(', ')} (see evidence; real-model runs are recorded as they happened)` };
    },
  },
  {
    // Runs last: its old-extension part registers a second relay for the profile.
    id: 15, name: 'Old journal and upgrade retry', timeout: 600,
    async run(h, ev) {
      h.assert.ok(process.env.OLD_BIN && fs.existsSync(process.env.OLD_BIN), 'needs the v1.5.214 binary (OLD_BIN)');
      const rig = h.rig;
      // 1. A v1.5.214 service in its own root writes the conversation.
      const oldHome = path.join(rig.work, 'home-old');
      const oldRoot = path.join(oldHome, '.chrome-use', 'agent-panel');
      const oldBin = path.join(rig.work, 'bin', 'chrome-use-v1.5.214');
      fs.copyFileSync(process.env.OLD_BIN, oldBin); fs.chmodSync(oldBin, 0o755);
      const oldJson = path.join(oldRoot, 'profiles', rig.profile, 'service.json');
      const { spawn } = await import('node:child_process');
      const proc = spawn(oldBin, ['__panel-service', '--profile', rig.profile, '--data-dir', oldRoot], { env: { ...rig.serviceEnv, HOME: oldHome }, stdio: 'ignore', detached: true });
      proc.unref();
      await h.waitFor(() => fs.existsSync(oldJson), { timeout: 20000, label: 'old service.json' });
      const old = new Service(oldJson);
      const conv = (await old.call('conversationCreate', { agentId: 'codex', executable: rig.backend('codex'), fullAccess: true })).conversation;
      const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
      const attachment = (await old.call('conversationAttachmentPut', { conversationId: conv.id, data: png })).attachment;
      const tok = `SYNTH_OLDTOK_${randomUUID().slice(0, 8)}`;
      const page = { tabId: 7, url: h.site(`/b?access_token=${tok}&x=1`), title: 'Page B' };
      const m1 = randomUUID();
      const text1 = h.scenario('c15old', { steps: [{ say: 'Old build reply.' }] });
      await old.call('conversationSend', { conversationId: conv.id, messageId: m1, text: text1, page, attachments: [attachment] });
      await h.waitFor(async () => { const r = await old.call('conversationRead', { conversationId: conv.id, after: 0 }); return r.journal.events.some(e => e.kind === 'terminal') && r.conversation.phase === 'idle'; }, { timeout: 60000, label: 'old turn done' });
      await old.tryCall('serviceStop', {});
      await h.waitFor(() => { try { process.kill(proc.pid, 0); return false; } catch { return true; } }, { timeout: 20000, label: 'old service exit' });
      const oldFolder = path.join(oldRoot, 'profiles', rig.profile, 'conversations', conv.id);
      const oldBytes = fs.readFileSync(path.join(oldFolder, 'events.jsonl'));
      ev.oldKinds = oldBytes.toString().trim().split('\n').map(l => JSON.parse(l).kind);
      // 2. Move it into this build's store while this build's service is stopped.
      const { stopService, startService } = await import('./rig.mjs');
      await stopService(rig);
      const store = path.join(rig.profileDir, 'conversations');
      fs.cpSync(oldFolder, path.join(store, conv.id), { recursive: true });
      const oldIndex = JSON.parse(fs.readFileSync(path.join(oldRoot, 'profiles', rig.profile, 'conversations', 'index.json'), 'utf8'));
      const indexPath = path.join(store, 'index.json');
      const index = JSON.parse(fs.readFileSync(indexPath, 'utf8'));
      index[conv.id] = oldIndex[conv.id];
      fs.writeFileSync(indexPath, JSON.stringify(index), { mode: 0o600 });
      await startService(rig);
      // 3. It opens; the retry with the same id is a duplicate (raw, then projection).
      const read = await h.journal(conv.id);
      ev.readKinds = read.events.map(e => e.kind);
      const oldUser = read.events.find(e => e.kind === 'user');
      ev.projectedPage = oldUser.data.page;
      const raw = await rig.service.tryCall('conversationSend', { conversationId: conv.id, messageId: m1, text: text1, page, attachments: [attachment] });
      const projected = await rig.service.tryCall('conversationSend', { conversationId: conv.id, messageId: m1, text: text1, page: oldUser.data.page, attachments: [attachment] });
      ev.rawRetry = raw.ok ? 'ok' : raw.error; ev.projectedRetry = projected.ok ? 'ok' : projected.error;
      h.assert.equal(raw.ok, true, 'raw retry after the upgrade is accepted as a duplicate');
      h.assert.equal(projected.ok, true, 'retry with the projected URL is accepted as a duplicate');
      // 4. A new message runs; the old lines are byte-identical.
      const m2 = await h.send(conv.id, h.scenario('c15new', { steps: [{ tool: 'chrome_use_open', args: { url: h.site('/a') } }, { say: 'New build reply.' }] }));
      const events = await h.turnEnd(conv.id, m2.messageId);
      h.assert.equal(events.filter(e => e.kind === 'user' && e.data.messageId === m1).length, 1, 'no second user event for the old message');
      const now = fs.readFileSync(path.join(store, conv.id, 'events.jsonl'));
      ev.oldLinesIdentical = now.subarray(0, oldBytes.length).equals(oldBytes);
      h.assert.ok(ev.oldLinesIdentical, 'old journal lines are byte-identical');
      ev.newTurn = { outcome: h.one(events, 'turnEnded', d => d.messageId === m2.messageId)?.data.outcome };
      h.assert.equal(ev.newTurn.outcome, 'completed');
      ev.panel = await h.screenshot('check15-upgraded', conv.id);
      h.panelCheck(ev, /not recorded/i.test(ev.panel.transcript), 'old turn shows "not recorded" labels');
      // 5. The v1.5.214 extension against this build: rows render, an approval can be answered.
      const pending = await h.create('codex', { fullAccess: false });
      const pm = await h.send(pending.id, h.scenario('c15approval', { steps: [{ request: { kind: 'command', tool: 'chrome_use_click' } }, { say: 'approval answered' }] }));
      await h.waitEvent(pending.id, 'request', () => true, 60000);
      const oldUd = path.join(rig.work, 'chrome-old');
      fs.mkdirSync(path.join(oldUd, 'NativeMessagingHosts'), { recursive: true });
      fs.writeFileSync(path.join(oldUd, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'old ext', path: path.join(rig.work, 'nm-host.sh'), type: 'stdio', allowed_origins: ['chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/'] }));
      const { launch, api } = await import('../inspect-acceptance/lib.mjs');
      const oldArgs = ['--headless=new', `--load-extension=${process.env.OLD_EXT_DIR}`, `--disable-extensions-except=${process.env.OLD_EXT_DIR}`, '--window-size=1280,900'];
      let b2 = await launch({ chrome: process.env.CHROME, port: 9372, userDataDir: oldUd, args: oldArgs });
      try {
        let A2 = api(b2);
        let sw = await A2.worker();
        ev.oldExtensionVersion = await A2.evaluate(sw.sid, 'chrome.runtime.getManifest().version');
        // Bind the old extension to this run's profile: write its storage, then
        // restart the browser on the same profile so its first hello carries it.
        await A2.evaluate(sw.sid, `chrome.storage.local.set({ ab_profile_id: ${JSON.stringify(rig.profile)}, ab_panel_conversation: ${JSON.stringify(conv.id)}, ab_panel_conversation_scope: 'all' })`);
        await h.sleep(1000);
        b2.close();
        await h.sleep(2000);
        b2 = await launch({ chrome: process.env.CHROME, port: 9372, userDataDir: oldUd, args: oldArgs });
        A2 = api(b2);
        sw = await A2.worker();
        // A worker attached while it starts can lack its extension bindings for a moment.
        ev.oldExtensionProfile = await h.waitFor(async () => (await A2.evaluate(sw.sid, `typeof chrome === 'object' && chrome.storage ? chrome.storage.local.get('ab_profile_id') : null`))?.ab_profile_id, { timeout: 20000, label: 'old extension storage' });
        h.assert.equal(ev.oldExtensionProfile, rig.profile);
        await h.sleep(4000);
        const openOld = async () => {
          const win = await A2.evaluate(sw.sid, 'chrome.windows.getLastFocused()');
          const { targetId } = await b2.send('Target.createTarget', { url: 'chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/sidepanel.html?opener=1' });
          const os = await A2.attach(targetId);
          await h.waitFor(() => A2.evaluate(os, 'document.readyState === "complete"'));
          await A2.evaluate(os, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.sidePanel.open({ windowId: ${win.id} }); (document.body || document.documentElement).append(b); return true; })()`);
          await A2.click(os, 50, 40);
          const t = await h.waitFor(async () => (await b2.send('Target.getTargets')).targetInfos.find(x => x.url.endsWith('/sidepanel.html')), { label: 'old side panel' });
          return A2.attach(t.targetId);
        };
        const ps = await openOld();
        await h.sleep(4000);
        const shot = async (name, sid) => { const s = await b2.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(h.out, `panel-${name}.png`), Buffer.from(s.data, 'base64')); return A2.evaluate(sid, `(document.getElementById('transcript') || document.body).innerText`); };
        // A journal written by this build with every evidence kind (check 7's or this one).
        const rich = h.state.c7?.conv.id || conv.id;
        await A2.evaluate(sw.sid, `chrome.storage.local.set({ ab_panel_conversation: ${JSON.stringify(rich)} })`);
        await b2.send('Page.reload', {}, ps); await h.sleep(4000);
        ev.oldPanelRich = (await shot('check15-oldext-newjournal', ps)).slice(0, 2000);
        h.assert.ok(!/\[object Object\]/.test(ev.oldPanelRich), 'v1.5.214 panel: no [object Object]');
        h.assert.ok(ev.oldPanelRich.length > 20, 'v1.5.214 panel rendered the journal');
        await A2.evaluate(sw.sid, `chrome.storage.local.set({ ab_panel_conversation: ${JSON.stringify(pending.id)} })`);
        await b2.send('Page.reload', {}, ps); await h.sleep(4000);
        ev.oldPanelApproval = (await shot('check15-oldext-approval', ps)).slice(0, 1500);
        const clicked = await A2.evaluate(ps, `(() => { const b = [...document.querySelectorAll('button')].find(x => /^(Allow|Approve|Accept)/i.test(x.textContent.trim())); if (!b) return null; b.click(); return b.textContent.trim(); })()`);
        ev.oldPanelClicked = clicked;
        h.assert.ok(clicked, 'v1.5.214 panel shows an Allow button');
        const pe = await h.turnEnd(pending.id, pm.messageId, 60000);
        const reply = h.backendLog().find(l => l.event === 'requestReply' && l.scenario === 'c15approval');
        ev.oldPanelReply = reply?.reply;
        h.assert.equal(reply?.reply?.allowed, true, 'the v1.5.214 panel answered the approval');
        h.assert.equal(h.one(pe, 'turnEnded', d => d.messageId === pm.messageId).data.outcome, 'completed');
      } finally { b2.close(); }
    },
  },
];
