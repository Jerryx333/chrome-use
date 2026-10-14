import assert from 'node:assert/strict';
import test from 'node:test';
import { ScheduleNotifications } from './schedule-notifications.js';

test('only confirmed terminal runs notify, once across repeated polls and worker restarts', async () => {
  const notices=[];let saved={};let now=100;
  const options={storage:{async get(){return saved;},async set(value){saved=structuredClone(value);}},notifications:{async create(id,value){notices.push({id,value});}},now:()=>now};
  const task={id:'schedule',name:'Daily check',lastChat:'chat',lastResult:'running',activeChat:'chat'};
  const read=async()=>({schedules:[task]});
  let notifier=new ScheduleNotifications(options);
  await notifier.poll(read);assert.equal(notices.length,0);
  task.finishedAt=120;task.lastResult='completed';task.activeChat=null;
  await notifier.poll(read);await notifier.poll(read);assert.equal(notices.length,1);
  now=200;notifier=new ScheduleNotifications(options);await notifier.poll(read);assert.equal(notices.length,1);
  task.lastChat='next-chat';task.finishedAt=220;task.lastResult='failed: fixture';
  await notifier.poll(read);assert.equal(notices.length,2);assert.equal(notices[1].value.message,'failed: fixture');
  assert.equal(JSON.stringify(saved).includes('Daily check'),false);
});

test('overlapping notification checks do not duplicate a terminal run', async () => {
  let resolve;const pause=new Promise(r=>{resolve=r;});let count=0;
  const notifier=new ScheduleNotifications({storage:{async get(){return {};},async set(){}},notifications:{async create(){count++;await pause;}},now:()=>100});
  const read=async()=>({schedules:[{id:'task',name:'Check',lastChat:'chat',lastResult:'completed',finishedAt:110}]});
  const first=notifier.poll(read);await new Promise(resolve=>setImmediate(resolve));
  await notifier.poll(read);assert.equal(count,1);resolve();await first;
});
