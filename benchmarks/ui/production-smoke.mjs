import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { installFixture } from './fixture.mjs';

// Production assets + the configured CSP, with only synthetic/native IPC mocks.
const dist = path.resolve('dist');
const config = JSON.parse(await fs.readFile('src-tauri/tauri.conf.json', 'utf8'));
const output = await fs.mkdtemp(path.join(os.tmpdir(), 'sotto-ui-production-'));
const { chromium } = await import(process.env.SOTTO_UI_PLAYWRIGHT ? pathToFileURL(path.resolve(process.env.SOTTO_UI_PLAYWRIGHT)).href : 'playwright');
const mime = {'.html':'text/html','.js':'text/javascript','.css':'text/css','.svg':'image/svg+xml','.png':'image/png','.ico':'image/x-icon'};
const server = createServer(async (request, response) => {
  try {
    const file = path.resolve(dist, `.${decodeURIComponent(new URL(request.url, 'http://localhost').pathname)}`);
    if (!file.startsWith(`${dist}${path.sep}`)) { response.writeHead(403).end(); return; }
    const content = await fs.readFile(file);
    response.writeHead(200, {'Content-Type':mime[path.extname(file)] ?? 'application/octet-stream','Content-Security-Policy':config.app.security.csp});
    response.end(content);
  } catch { response.writeHead(404).end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({headless:true,...(process.env.SOTTO_UI_BROWSER_EXECUTABLE ? {executablePath:process.env.SOTTO_UI_BROWSER_EXECUTABLE} : {channel:'chrome'})});

function installFlows({statusError=false, settingsError=false}={}) {
  const original = window.__TAURI_INTERNALS__.invoke;
  const status = {available:true,unavailable_reason:null,downloaded:false,downloading:false,loaded:false,preparing:false,setup_error:null,model_name:'Synthetic cleanup model',model_path:null,model_url:'https://example.invalid/model',download_size_mb:227,update_available:false,last_cleanup_status:{kind:'idle'}};
  window.__flow = {prepareCalls:0,saved:[],violations:[],opened:[],copies:[],copyError:false};
  document.addEventListener('securitypolicyviolation',(event)=>window.__flow.violations.push({directive:event.effectiveDirective,blocked:event.blockedURI}));
  let pending;
  window.__finishPreparation = () => { if (pending) {Object.assign(status,{preparing:false,downloaded:true,loaded:true});pending.resolve(structuredClone(status));pending=null;window.__emit('llm-preparation-changed',null);} };
  window.__TAURI_INTERNALS__.invoke = async (command,args={}) => {
    if (command==='get_settings' && settingsError) throw new Error('Synthetic settings read failure');
    if (command==='get_llm_status') {if(statusError)throw new Error('Synthetic model status failure');return structuredClone(status);}
    if (command==='prepare_llm_model') {
      window.__flow.prepareCalls++;
      if(status.loaded)return structuredClone(status);
      if(!pending){let resolve;const promise=new Promise((done)=>{resolve=done;});pending={promise,resolve};status.preparing=true;window.__emit('llm-preparation-changed',null);}
      return pending.promise;
    }
    if (command==='update_settings') window.__flow.saved.push(structuredClone(args.newSettings));
    if (command==='open_url') {window.__flow.opened.push(args.url);return;}
    if (command==='plugin:clipboard-manager|write_text') {
      if(window.__flow.copyError)throw new Error('Synthetic clipboard failure');
      window.__flow.copies.push(args.text);return;
    }
    if (['reveal_recording_audio','dismiss_overlay_error','open_transcription_history'].includes(command)) return;
    return original(command,args);
  };
}

const pages=[];
async function newPage(name,fixture={},flows={}) {
  const context=await browser.newContext({viewport:{width:520,height:600}});
  await context.route('**/*',(route)=>new URL(route.request().url()).origin===origin ? route.continue() : route.abort());
  await context.addInitScript({content:`(${installFixture.toString()})(${JSON.stringify(fixture)});(${installFlows.toString()})(${JSON.stringify(flows)});`});
  const page=await context.newPage();page.setDefaultTimeout(10_000);
  await page.goto(`${origin}/${name}.html`);pages.push({name,page,context});return page;
}
const report={browser:await browser.version(),mode:'Production dist; configured CSP; synthetic IPC',csp:config.app.security.csp,checks:[]};
try {
  const settings=await newPage('settings');await settings.getByText('All changes saved',{exact:true}).waitFor();
  for(const section of ['General','Dictation','Vocabulary','Advanced']) {
    await settings.getByRole('tab',{name:section,exact:true}).click();
    await settings.getByRole('tabpanel').waitFor();
    assert.equal(await settings.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    await settings.screenshot({path:path.join(output,`settings-${section.toLowerCase()}.png`)});
  }
  await settings.getByRole('link',{name:'Model details'}).click();assert.equal(await settings.evaluate(()=>window.__flow.opened.length),1);
  await settings.getByRole('tab',{name:'Dictation',exact:true}).click();
  const cleanup=settings.getByRole('checkbox',{name:/AI cleanup suggestions/});assert.equal(await cleanup.isChecked(),false);
  await cleanup.click();await settings.getByRole('button',{name:'Cancel setup',exact:true}).waitFor();
  assert.equal(await settings.evaluate(()=>window.__flow.prepareCalls),1);
  await settings.getByRole('button',{name:'Cancel setup',exact:true}).click();await settings.evaluate(()=>window.__finishPreparation());
  await settings.getByText(/Activation cancelled/).waitFor();assert.equal(await cleanup.isChecked(),false);
  assert.equal(await settings.evaluate(()=>window.__flow.saved.length),0);
  await cleanup.click();await settings.getByText('Ready. Save to enable AI suggestions.',{exact:true}).waitFor();
  await settings.getByRole('button',{name:'Save',exact:true}).click();
  await settings.waitForFunction(()=>window.__flow.saved.length===1);
  assert.equal(await settings.evaluate(()=>window.__flow.saved[0].llm_cleanup_enabled),true);
  report.checks.push('four sections, native model link, default-off, automatic preparation, cancelled late activation, explicit save');

  const statusFailure=await newPage('settings',{}, {statusError:true});await statusFailure.getByRole('tab',{name:'Dictation',exact:true}).click();
  await statusFailure.getByText(/Synthetic model status failure/).waitFor();
  const settingsFailure=await newPage('settings',{}, {settingsError:true});await settingsFailure.getByText(/Synthetic settings read failure/).waitFor();
  assert.equal(await settingsFailure.getByRole('button',{name:'Save',exact:true}).isDisabled(),true);
  report.checks.push('visible model/settings read errors, persistence disabled on load failure');

  const history=await newPage('history',{historyCount:5000,label:'history'});
  await history.waitForFunction(()=>document.querySelectorAll('.history-item').length===50);
  await history.getByRole('searchbox').fill('Item 4999');await history.waitForFunction(()=>document.querySelectorAll('.history-item').length===1);
  report.checks.push('bounded history and search across all5000 entries');
  await history.evaluate(()=>window.__emit('transcription-complete',{
    id:'synthetic-suggestion',created_at:'2099-01-01T00:00:00Z',duration_ms:1000,word_count:9,
    text:'Qwen said the word um in the note.',raw_text:'Quen said the word um in the note.',
    cleanup_suggestion:'Qwen said the word in the note.',llm_applied:false,
    llm_cleanup_status:{kind:'suggested',detail:{elapsed_ms:20}},removed_ids:[],
  }));
  await history.getByRole('searchbox').fill('Qwen');await history.locator('.item-body').click();
  await history.getByRole('button',{name:'Diff',exact:true}).click();
  await history.getByRole('button',{name:'Copy transcript',exact:true}).click();
  assert.equal(await history.evaluate(()=>window.__flow.copies.at(-1)),'Qwen said the word um in the note.');
  await history.evaluate(()=>{window.__flow.copyError=true;});
  await history.getByRole('button',{name:'Copy suggestion',exact:true}).click();
  await history.getByText(/Synthetic clipboard failure/).waitFor();
  assert.equal(await history.getByText('Suggestion copied',{exact:true}).count(),0);
  await history.evaluate(()=>{window.__flow.copyError=false;});
  await history.getByRole('button',{name:'Copy suggestion',exact:true}).click();
  await history.getByText('Suggestion copied',{exact:true}).waitFor();
  assert.equal(await history.evaluate(()=>window.__flow.copies.at(-1)),'Qwen said the word in the note.');
  assert.equal(await history.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  await history.screenshot({path:path.join(output,'history-suggestion.png')});
  report.checks.push('separate provenance/suggestion diffs, preserved ordinary Copy, explicit suggestion Copy and truthful clipboard errors');

  const overlay=await newPage('overlay',{label:'overlay'});await overlay.setViewportSize({width:300,height:110});
  await overlay.waitForFunction(()=>window.__uiBench.ipc.some((entry)=>entry.command==='get_overlay_snapshot'));
  await overlay.evaluate(()=>window.__emit('overlay-state',{revision:1,generation:1,state:'Idle',started_at_ms:null,error:{event:'transcription-error',payload:{generation:1,error:'Synthetic failure: retained audio is available.',audio_path:'/tmp/sotto_00000000-0000-0000-0000-000000000000.wav'}}}));
  await overlay.getByRole('button',{name:'Show audio',exact:true}).waitFor();
  await overlay.waitForFunction(()=>getComputedStyle(document.querySelector('.pill-container')).opacity==='1');
  assert.equal(await overlay.evaluate(()=>document.documentElement.scrollHeight>innerHeight),false);
  await overlay.screenshot({path:path.join(output,'overlay-error.png')});
  await overlay.emulateMedia({reducedMotion:'reduce'});
  await overlay.evaluate(()=>window.__emit('overlay-state',{revision:2,generation:2,state:'Recording',started_at_ms:Date.now()-12_000,error:null}));
  await overlay.getByText('0:12',{exact:true}).waitFor();
  assert.equal(await overlay.locator('.recording-dot').evaluate((element)=>getComputedStyle(element).animationName),'none');
  report.checks.push('generation snapshot error,300×110 layout, reduced motion and recovered elapsed time');

  for(const {name,page} of pages){
    const result=await page.evaluate(()=>({errors:window.__uiBench.errors,unknownCommands:window.__uiBench.unknownCommands,violations:window.__flow.violations}));
    assert.deepEqual(result,{errors:[],unknownCommands:[],violations:[]},`${name} runtime/CSP failures`);
  }
  await fs.writeFile(path.join(output,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));console.error(`Production UI artifacts: ${output}`);
} finally {
  await browser.close();await new Promise((resolve)=>server.close(resolve));
}
