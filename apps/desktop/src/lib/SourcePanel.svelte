<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { Check, CirclePause, Copy, ExternalLink, Plus, Radio, RefreshCw, ShieldCheck, Trash2, TriangleAlert } from '@lucide/svelte';
  import { onMount } from 'svelte';
  import type { DeviceAuthorizationPrompt, SourceRuntimeState, SourceView, TwitchAuthorizationStatus, TwitchSourceTest } from './contracts';

  let { inTauri, onEnsureSession }: { inTauri: boolean; onEnsureSession: () => Promise<string> } = $props();
  let sources = $state<SourceView[]>([]);
  let displayName = $state('Chat Twitch principal');
  let clientId = $state('');
  let busy = $state('');
  let error = $state('');
  let notice = $state('');
  let authSourceId = $state('');
  let authPrompt = $state<DeviceAuthorizationPrompt | null>(null);
  let testedIdentity = $state<Record<string, TwitchSourceTest>>({});
  let pollGeneration = 0;

  onMount(() => {
    if (!inTauri) return;
    void loadSources();
    const refresh = window.setInterval(() => void loadSources(true), 2_000);
    return () => { window.clearInterval(refresh); pollGeneration += 1; };
  });

  async function loadSources(silent = false) {
    if (!inTauri || (busy && !silent)) return;
    try { sources = await invoke<SourceView[]>('list_sources_ipc'); }
    catch (cause) { if (!silent) error = readableError(cause); }
  }

  async function createSource() {
    if (!displayName.trim() || !clientId.trim() || busy) return;
    busy = 'create'; error = ''; notice = '';
    try {
      const source = await invoke<SourceView>('create_twitch_source_ipc', { displayName: displayName.trim(), clientId: clientId.trim() });
      sources = [...sources, source];
      notice = 'Source créée en pause. Autorisez maintenant votre compte Twitch.';
      busy = '';
      await beginAuthorization(source);
    } catch (cause) { error = readableError(cause); busy = ''; }
  }

  async function beginAuthorization(source: SourceView) {
    if (busy) return;
    busy = `auth-${source.source_id}`; error = ''; notice = ''; pollGeneration += 1;
    const generation = pollGeneration;
    try {
      authPrompt = await invoke<DeviceAuthorizationPrompt>('begin_twitch_authorization_ipc', { sourceId: source.source_id });
      authSourceId = source.source_id;
      notice = 'Ouvrez Twitch, saisissez le code, puis revenez ici. La vérification est automatique.';
      void pollAuthorization(source.source_id, generation);
    } catch (cause) { error = readableError(cause); busy = ''; }
  }

  async function pollAuthorization(sourceId: string, generation: number) {
    while (generation === pollGeneration && authSourceId === sourceId && authPrompt && Date.now() < authPrompt.expires_at_ms) {
      await delay(Math.max(1, authPrompt.poll_interval_seconds) * 1_000);
      if (generation !== pollGeneration) return;
      try {
        const status = await invoke<TwitchAuthorizationStatus>('poll_twitch_authorization_ipc', { sourceId });
        if (status.status === 'authorized') {
          testedIdentity = { ...testedIdentity, [sourceId]: status.identity };
          authPrompt = null; authSourceId = ''; busy = '';
          notice = `Compte @${status.identity.login} autorisé. La source est prête.`;
          await loadSources(true); return;
        }
        authPrompt = status.status === 'slow_down'
          ? { ...status.prompt, poll_interval_seconds: Math.min(status.prompt.poll_interval_seconds + 5, 60) }
          : status.prompt;
      } catch (cause) {
        error = readableError(cause); busy = ''; authPrompt = null; authSourceId = ''; return;
      }
    }
    if (generation === pollGeneration && authPrompt) {
      error = 'Le code Twitch a expiré. Relancez l’autorisation.'; busy = ''; authPrompt = null; authSourceId = '';
    }
  }

  async function testSource(source: SourceView) {
    if (busy) return;
    busy = `test-${source.source_id}`; error = '';
    try {
      const identity = await invoke<TwitchSourceTest>('test_twitch_source_ipc', { sourceId: source.source_id });
      testedIdentity = { ...testedIdentity, [source.source_id]: identity };
      notice = `Connexion valide pour @${identity.login}.`;
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function startSource(source: SourceView) {
    if (busy) return;
    busy = `start-${source.source_id}`; error = '';
    try {
      const sessionId = await onEnsureSession();
      replaceSource(await invoke<SourceView>('start_twitch_source_ipc', { sourceId: source.source_id, expectedRevision: source.revision, sessionId }));
      notice = 'Connexion Twitch lancée. Les messages alimentent la session active.';
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function stopSource(source: SourceView) {
    if (busy) return;
    busy = `stop-${source.source_id}`; error = '';
    try { replaceSource(await invoke<SourceView>('stop_source_ipc', { sourceId: source.source_id })); notice = 'Source mise en pause.'; }
    catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function deleteSource(source: SourceView) {
    if (busy || source.desired_state !== 'paused' || !window.confirm(`Supprimer « ${source.display_name} » et son jeton du coffre système ?`)) return;
    busy = `delete-${source.source_id}`; error = '';
    try {
      await invoke('delete_source_ipc', { sourceId: source.source_id, expectedRevision: source.revision });
      sources = sources.filter((item) => item.source_id !== source.source_id);
      delete testedIdentity[source.source_id]; testedIdentity = { ...testedIdentity };
      notice = 'Source et identifiant local supprimés.';
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function copy(value: string, label: string) {
    try { await navigator.clipboard.writeText(value); notice = `${label} copié.`; }
    catch { error = 'La copie automatique a échoué. Sélectionnez la valeur manuellement.'; }
  }

  function replaceSource(source: SourceView) { sources = sources.map((item) => item.source_id === source.source_id ? source : item); }
  function stateLabel(state: SourceRuntimeState | null) {
    return ({ paused: 'En pause', authentication_required: 'Autorisation requise', connecting: 'Connexion…', connected: 'Connectée', backoff: 'Reconnexion…', faulted: 'Action requise' } as const)[state ?? 'paused'];
  }
  function readableError(cause: unknown) { return cause instanceof Error ? cause.message : String(cause); }
  function delay(milliseconds: number) { return new Promise((resolve) => window.setTimeout(resolve, milliseconds)); }
</script>

<section class="source-panel" aria-labelledby="source-heading">
  <div class="source-heading">
    <div class="source-title"><span class="source-icon"><Radio size={20} /></span><div><p class="eyebrow">Sources de chat</p><h2 id="source-heading">Brancher Twitch sans coupler le moteur</h2><p>EventSub traduit chaque message en soumission locale. Les jetons restent dans le coffre du système.</p></div></div>
    <span class="privacy"><ShieldCheck size={15} /> Aucun chat brut conservé</span>
  </div>

  <div class="source-create">
    <label>Nom de la source<input bind:value={displayName} maxlength="80" placeholder="Chat Twitch principal" /></label>
    <label>Client ID Twitch<input bind:value={clientId} maxlength="128" autocomplete="off" spellcheck="false" placeholder="Identifiant public de votre application Twitch" /></label>
    <button onclick={createSource} disabled={!inTauri || !!busy || !displayName.trim() || !clientId.trim()}><Plus size={16} /> {busy === 'create' ? 'Création…' : 'Ajouter Twitch'}</button>
  </div>

  {#if authPrompt}
    <div class="authorization" aria-live="polite">
      <div><span>Code d’autorisation</span><strong>{authPrompt.user_code}</strong><small>Expire dans {Math.max(0, Math.ceil((authPrompt.expires_at_ms - Date.now()) / 60_000))} min</small></div>
      <div class="authorization-actions"><a href={authPrompt.verification_uri} target="_blank" rel="noreferrer">Ouvrir Twitch <ExternalLink size={15} /></a><button onclick={() => copy(authPrompt!.user_code, 'Code')}><Copy size={15} /> Code</button><button onclick={() => copy(authPrompt!.verification_uri, 'Lien')}><Copy size={15} /> Lien</button></div>
    </div>
  {/if}

  {#if sources.length}
    <div class="source-list">
      {#each sources as source (source.source_id)}
        <article class:connected={source.runtime.state === 'connected'} class:faulted={source.runtime.state === 'faulted'}>
          <div class="source-summary"><span class="state-dot"></span><div><strong>{source.display_name}</strong><small>{testedIdentity[source.source_id] ? `@${testedIdentity[source.source_id].login}` : 'Twitch EventSub'}</small></div><span class="state-label">{stateLabel(source.runtime.state)}</span></div>
          <dl><div><dt>Messages</dt><dd>{source.runtime.messages_received}</dd></div><div><dt>Acceptés</dt><dd>{source.runtime.accepted}</dd></div><div><dt>Session</dt><dd>{source.runtime.session_id?.slice(0, 8) ?? '—'}</dd></div></dl>
          {#if source.runtime.detail}<p class="source-detail">Code : {source.runtime.detail}</p>{/if}
          <div class="source-actions">
            {#if !source.authenticated}<button onclick={() => beginAuthorization(source)} disabled={!!busy}><ShieldCheck size={15} /> Autoriser</button>
            {:else}<button onclick={() => testSource(source)} disabled={!!busy || source.desired_state === 'active'}><RefreshCw size={15} /> Tester</button>{#if source.desired_state === 'active'}<button class="pause" onclick={() => stopSource(source)} disabled={!!busy}><CirclePause size={15} /> Pause</button>{:else}<button class="start" onclick={() => startSource(source)} disabled={!!busy}><Radio size={15} /> Écouter</button>{/if}{/if}
            <button class="delete" aria-label={`Supprimer ${source.display_name}`} onclick={() => deleteSource(source)} disabled={!!busy || source.desired_state === 'active'}><Trash2 size={15} /></button>
          </div>
        </article>
      {/each}
    </div>
  {:else}<div class="source-empty"><Radio size={23} /><span><strong>Aucune source configurée</strong><small>L’application reste entièrement utilisable en saisie manuelle.</small></span></div>{/if}
  {#if notice}<p class="source-notice" aria-live="polite"><Check size={15} /> {notice}</p>{/if}
  {#if error}<p class="source-error" role="alert"><TriangleAlert size={16} /> {error}</p>{/if}
</section>

<style>
  .source-panel{margin-top:20px;border:1px solid var(--line);border-radius:18px;background:color-mix(in srgb,var(--surface) 94%,#9146ff 6%);overflow:hidden}.source-heading{display:flex;justify-content:space-between;gap:24px;padding:24px;border-bottom:1px solid var(--line)}.source-title{display:flex;gap:14px;align-items:flex-start}.source-title h2{margin:2px 0 6px;font-size:1.2rem}.source-title p:last-child{margin:0;color:var(--muted);max-width:760px}.source-icon{display:grid;place-items:center;width:40px;height:40px;border-radius:12px;background:#9146ff1c;color:#a970ff}.privacy{display:flex;align-items:center;gap:7px;color:var(--muted);font-size:.78rem;white-space:nowrap}.source-create{display:grid;grid-template-columns:minmax(180px,.8fr) minmax(260px,1.2fr) auto;gap:12px;align-items:end;padding:18px 24px;border-bottom:1px solid var(--line)}label{display:grid;gap:7px;color:var(--muted);font-size:.76rem;font-weight:650}input{min-width:0;border:1px solid var(--line);border-radius:10px;background:var(--background);color:var(--text);padding:11px 12px}button,a{display:inline-flex;align-items:center;justify-content:center;gap:7px;min-height:40px;border-radius:10px;border:1px solid var(--line);background:var(--surface);color:var(--text);padding:9px 13px;font:inherit;font-size:.8rem;font-weight:700;text-decoration:none;cursor:pointer}button:disabled{opacity:.48;cursor:not-allowed}.source-create>button,.start{border-color:#9146ff66;background:#9146ff;color:#fff}.authorization{display:flex;justify-content:space-between;gap:20px;padding:18px 24px;background:#9146ff12;border-bottom:1px solid #9146ff35}.authorization>div:first-child{display:grid;gap:2px}.authorization span,.authorization small{color:var(--muted);font-size:.72rem}.authorization strong{font:750 1.45rem/1.2 ui-monospace,monospace;letter-spacing:.12em}.authorization-actions{display:flex;align-items:center;flex-wrap:wrap;gap:8px}.authorization-actions a{background:#9146ff;border-color:#9146ff;color:#fff}.source-list{display:grid;grid-template-columns:repeat(auto-fit,minmax(290px,1fr));gap:12px;padding:18px 24px}article{border:1px solid var(--line);border-radius:14px;background:var(--background);padding:16px}article.connected{border-color:#39c98166}article.faulted{border-color:#f06d6d70}.source-summary{display:grid;grid-template-columns:auto 1fr auto;align-items:center;gap:10px}.source-summary>div{display:grid}.source-summary small{color:var(--muted);font-size:.72rem}.state-dot{width:9px;height:9px;border-radius:50%;background:#8c929b;box-shadow:0 0 0 4px #8c929b20}article.connected .state-dot{background:#39c981;box-shadow:0 0 0 4px #39c98120}article.faulted .state-dot{background:#f06d6d;box-shadow:0 0 0 4px #f06d6d20}.state-label{color:var(--muted);font-size:.72rem}dl{display:grid;grid-template-columns:repeat(3,1fr);margin:14px 0;border-block:1px solid var(--line)}dl div{padding:10px 4px}dt{color:var(--muted);font-size:.66rem}dd{margin:3px 0 0;font:700 .82rem ui-monospace,monospace}.source-detail{margin:-4px 0 12px;color:#e48d8d;font:.7rem ui-monospace,monospace}.source-actions{display:flex;flex-wrap:wrap;gap:8px}.source-actions .delete{margin-left:auto;color:#e48d8d;padding-inline:11px}.source-actions .pause{color:#e6b76e}.source-empty{display:flex;align-items:center;justify-content:center;gap:12px;padding:26px;color:var(--muted)}.source-empty span{display:grid}.source-empty strong{color:var(--text)}.source-empty small{margin-top:2px}.source-notice,.source-error{display:flex;align-items:center;gap:8px;margin:0;padding:12px 24px;border-top:1px solid var(--line);font-size:.8rem}.source-notice{color:#55d99a}.source-error{color:#ef8f8f}@media(max-width:760px){.source-heading,.authorization{flex-direction:column}.privacy{white-space:normal}.source-create{grid-template-columns:1fr}.source-list{grid-template-columns:1fr}}
</style>
