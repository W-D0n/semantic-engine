<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { Check, CirclePause, Copy, ExternalLink, ListVideo, Plus, Radio, RefreshCw, ShieldCheck, Trash2, TriangleAlert } from '@lucide/svelte';
  import { onMount } from 'svelte';
  import type { BrowserAuthorizationPrompt, DeviceAuthorizationPrompt, SourceDeletionReceipt, SourceRuntimeState, SourceView, TwitchAuthorizationStatus, TwitchSourceTest, YouTubeAuthorizationStatus, YouTubeBroadcast, YouTubeSourceTest } from './contracts';

  let { inTauri, onEnsureSession }: { inTauri: boolean; onEnsureSession: () => Promise<string> } = $props();
  let sources = $state<SourceView[]>([]);
  let displayName = $state('Chat Twitch principal');
  let platform = $state<'twitch' | 'youtube'>('twitch');
  let clientId = $state('');
  let videoId = $state('');
  let policyAcknowledged = $state(false);
  let busy = $state('');
  let error = $state('');
  let notice = $state('');
  let authSourceId = $state('');
  let authPrompt = $state<DeviceAuthorizationPrompt | null>(null);
  let browserPrompt = $state<BrowserAuthorizationPrompt | null>(null);
  let testedIdentity = $state<Record<string, string>>({});
  let broadcastsBySource = $state<Record<string, YouTubeBroadcast[]>>({});
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
    if (!displayName.trim() || !clientId.trim() || busy || (platform === 'youtube' && !policyAcknowledged)) return;
    busy = 'create'; error = ''; notice = '';
    try {
      const source = platform === 'twitch'
        ? await invoke<SourceView>('create_twitch_source_ipc', { displayName: displayName.trim(), clientId: clientId.trim() })
        : await invoke<SourceView>('create_youtube_source_ipc', { displayName: displayName.trim(), clientId: clientId.trim(), videoId: videoId.trim(), policyAcknowledged });
      sources = [...sources, source];
      notice = `Source ${platform === 'twitch' ? 'Twitch' : 'YouTube'} créée en pause. Autorisez maintenant votre compte.`;
      busy = '';
      await beginAuthorization(source);
    } catch (cause) { error = readableError(cause); busy = ''; }
  }

  async function beginAuthorization(source: SourceView) {
    if (busy) return;
    busy = `auth-${source.source_id}`; error = ''; notice = ''; pollGeneration += 1;
    const generation = pollGeneration;
    try {
      if (isYouTube(source)) {
        browserPrompt = await invoke<BrowserAuthorizationPrompt>('begin_youtube_authorization_ipc', { sourceId: source.source_id });
        await openYouTubeAuthorization(browserPrompt.authorization_uri);
      } else {
        authPrompt = await invoke<DeviceAuthorizationPrompt>('begin_twitch_authorization_ipc', { sourceId: source.source_id });
      }
      authSourceId = source.source_id;
      notice = isYouTube(source) ? 'Terminez l’autorisation Google dans votre navigateur. Le retour est automatique.' : 'Ouvrez Twitch, saisissez le code, puis revenez ici. La vérification est automatique.';
      void pollAuthorization(source, generation);
    } catch (cause) { error = readableError(cause); busy = ''; }
  }

  async function pollAuthorization(source: SourceView, generation: number) {
    const sourceId = source.source_id;
    while (generation === pollGeneration && authSourceId === sourceId && (authPrompt || browserPrompt) && Date.now() < (authPrompt?.expires_at_ms ?? browserPrompt!.expires_at_ms)) {
      await delay(isYouTube(source) ? 1_000 : Math.max(1, authPrompt!.poll_interval_seconds) * 1_000);
      if (generation !== pollGeneration) return;
      try {
        if (isYouTube(source)) {
          const status = await invoke<YouTubeAuthorizationStatus>('poll_youtube_authorization_ipc', { sourceId });
          if (status.status === 'authorized') {
            testedIdentity = { ...testedIdentity, [sourceId]: status.identity.display_name };
            browserPrompt = null; authSourceId = ''; busy = '';
            notice = `Chaîne ${status.identity.display_name} autorisée. La source est prête.`;
            await loadSources(true); return;
          }
          browserPrompt = status.prompt;
          continue;
        }
        const status = await invoke<TwitchAuthorizationStatus>('poll_twitch_authorization_ipc', { sourceId });
        if (status.status === 'authorized') {
          testedIdentity = { ...testedIdentity, [sourceId]: `@${status.identity.login}` };
          authPrompt = null; authSourceId = ''; busy = '';
          notice = `Compte @${status.identity.login} autorisé. La source est prête.`;
          await loadSources(true); return;
        }
        authPrompt = status.status === 'slow_down'
          ? { ...status.prompt, poll_interval_seconds: Math.min(status.prompt.poll_interval_seconds + 5, 60) }
          : status.prompt;
      } catch (cause) {
        error = readableError(cause); busy = ''; authPrompt = null; browserPrompt = null; authSourceId = ''; return;
      }
    }
    if (generation === pollGeneration && (authPrompt || browserPrompt)) {
      error = 'L’autorisation a expiré. Relancez-la.'; busy = ''; authPrompt = null; browserPrompt = null; authSourceId = '';
    }
  }

  async function testSource(source: SourceView) {
    if (busy) return;
    busy = `test-${source.source_id}`; error = '';
    try {
      if (isYouTube(source)) {
        const identity = await invoke<YouTubeSourceTest>('test_youtube_source_ipc', { sourceId: source.source_id });
        testedIdentity = { ...testedIdentity, [source.source_id]: identity.display_name };
        notice = `Connexion YouTube valide pour ${identity.display_name}.`;
      } else {
        const identity = await invoke<TwitchSourceTest>('test_twitch_source_ipc', { sourceId: source.source_id });
        testedIdentity = { ...testedIdentity, [source.source_id]: `@${identity.login}` };
        notice = `Connexion Twitch valide pour @${identity.login}.`;
      }
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function startSource(source: SourceView) {
    if (busy) return;
    busy = `start-${source.source_id}`; error = '';
    try {
      const sessionId = await onEnsureSession();
      const command = isYouTube(source) ? 'start_youtube_source_ipc' : 'start_twitch_source_ipc';
      replaceSource(await invoke<SourceView>(command, { sourceId: source.source_id, expectedRevision: source.revision, sessionId }));
      notice = `Connexion ${isYouTube(source) ? 'YouTube' : 'Twitch'} lancée. Les messages alimentent la session active.`;
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function discoverBroadcasts(source: SourceView) {
    if (busy || source.desired_state === 'active') return;
    busy = `discover-${source.source_id}`; error = ''; notice = '';
    try {
      const broadcasts = await invoke<YouTubeBroadcast[]>('discover_youtube_broadcasts_ipc', { sourceId: source.source_id });
      broadcastsBySource = { ...broadcastsBySource, [source.source_id]: broadcasts };
      notice = broadcasts.length
        ? `${broadcasts.length} live${broadcasts.length > 1 ? 's' : ''} actif${broadcasts.length > 1 ? 's' : ''} trouvé${broadcasts.length > 1 ? 's' : ''}.`
        : 'Aucun live actif trouvé sur la chaîne autorisée.';
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function selectBroadcast(source: SourceView, broadcast: YouTubeBroadcast) {
    if (busy || source.desired_state === 'active') return;
    busy = `select-${source.source_id}`; error = ''; notice = '';
    try {
      replaceSource(await invoke<SourceView>('select_youtube_broadcast_ipc', {
        sourceId: source.source_id,
        expectedRevision: source.revision,
        videoId: broadcast.video_id,
      }));
      broadcastsBySource = { ...broadcastsBySource, [source.source_id]: [] };
      notice = `Live « ${broadcast.title} » sélectionné. La source peut maintenant être écoutée.`;
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
      const receipt = await invoke<SourceDeletionReceipt>('delete_source_ipc', { sourceId: source.source_id, expectedRevision: source.revision });
      sources = sources.filter((item) => item.source_id !== source.source_id);
      delete testedIdentity[source.source_id]; testedIdentity = { ...testedIdentity };
      delete broadcastsBySource[source.source_id]; broadcastsBySource = { ...broadcastsBySource };
      notice = receipt.provider_revocation === 'failed'
        ? 'Source et jeton local supprimés. La révocation distante a échoué : retirez aussi l’accès depuis les paramètres du fournisseur.'
        : receipt.provider_revocation === 'succeeded'
          ? 'Source, jeton local et accès fournisseur supprimés.'
          : 'Source et données locales supprimées.';
    } catch (cause) { error = readableError(cause); }
    finally { busy = ''; }
  }

  async function copy(value: string, label: string) {
    try { await navigator.clipboard.writeText(value); notice = `${label} copié.`; }
    catch { error = 'La copie automatique a échoué. Sélectionnez la valeur manuellement.'; }
  }

  async function openYouTubeAuthorization(authorizationUri: string) {
    try { await invoke('open_youtube_authorization_ipc', { authorizationUri }); }
    catch (cause) { error = readableError(cause); }
  }

  function replaceSource(source: SourceView) { sources = sources.map((item) => item.source_id === source.source_id ? source : item); }
  function isYouTube(source: SourceView) { return source.adapter === 'youtube-live-chat'; }
  function stateLabel(state: SourceRuntimeState | null) {
    return ({ paused: 'En pause', authentication_required: 'Autorisation requise', connecting: 'Connexion…', connected: 'Connectée', backoff: 'Reconnexion…', faulted: 'Action requise' } as const)[state ?? 'paused'];
  }
  function readableError(cause: unknown) { return cause instanceof Error ? cause.message : String(cause); }
  function delay(milliseconds: number) { return new Promise((resolve) => window.setTimeout(resolve, milliseconds)); }
</script>

<section class="source-panel" aria-labelledby="source-heading">
  <div class="source-heading">
    <div class="source-title"><span class="source-icon"><Radio size={20} /></span><div><p class="eyebrow">Sources de chat</p><h2 id="source-heading">Brancher Twitch ou YouTube sans coupler le moteur</h2><p>Chaque adaptateur traduit le chat en soumissions locales. Les jetons restent dans le coffre du système.</p></div></div>
    <span class="privacy"><ShieldCheck size={15} /> Aucun chat brut conservé</span>
  </div>

  <div class="source-create">
    <label>Plateforme<select bind:value={platform} onchange={() => { displayName = platform === 'twitch' ? 'Chat Twitch principal' : 'Live YouTube principal'; }}><option value="twitch">Twitch</option><option value="youtube">YouTube Live (expérimental)</option></select></label>
    <label>Nom de la source<input bind:value={displayName} maxlength="80" placeholder="Chat Twitch principal" /></label>
    <label>Client ID {platform === 'twitch' ? 'Twitch' : 'Google Desktop OAuth'}<input bind:value={clientId} maxlength="256" autocomplete="off" spellcheck="false" placeholder={platform === 'twitch' ? 'Identifiant public de votre application Twitch' : '…apps.googleusercontent.com'} /></label>
    {#if platform === 'youtube'}
      <label>ID de la vidéo live (optionnel)<input bind:value={videoId} maxlength="11" autocomplete="off" spellcheck="false" placeholder="Détection après autorisation" /></label>
      <label class="policy"><input type="checkbox" bind:checked={policyAcknowledged} /> J’ai lu les règles YouTube API. Verdict/score reste verrouillé par la distribution jusqu’à validation de conformité.</label>
    {/if}
    <button onclick={createSource} disabled={!inTauri || !!busy || !displayName.trim() || !clientId.trim() || (platform === 'youtube' && !policyAcknowledged)}><Plus size={16} /> {busy === 'create' ? 'Création…' : `Ajouter ${platform === 'twitch' ? 'Twitch' : 'YouTube'}`}</button>
  </div>

  {#if authPrompt}
    <div class="authorization" aria-live="polite">
      <div><span>Code d’autorisation</span><strong>{authPrompt.user_code}</strong><small>Expire dans {Math.max(0, Math.ceil((authPrompt.expires_at_ms - Date.now()) / 60_000))} min</small></div>
      <div class="authorization-actions"><a href={authPrompt.verification_uri} target="_blank" rel="noreferrer">Ouvrir Twitch <ExternalLink size={15} /></a><button onclick={() => copy(authPrompt!.user_code, 'Code')}><Copy size={15} /> Code</button><button onclick={() => copy(authPrompt!.verification_uri, 'Lien')}><Copy size={15} /> Lien</button></div>
    </div>
  {/if}

  {#if browserPrompt}
    <div class="authorization" aria-live="polite">
      <div><span>Autorisation Google ouverte dans le navigateur</span><small>Expire dans {Math.max(0, Math.ceil((browserPrompt.expires_at_ms - Date.now()) / 60_000))} min</small></div>
      <div class="authorization-actions"><button onclick={() => openYouTubeAuthorization(browserPrompt!.authorization_uri)}>Ouvrir Google <ExternalLink size={15} /></button><button onclick={() => copy(browserPrompt!.authorization_uri, 'Lien')}><Copy size={15} /> Lien</button></div>
    </div>
  {/if}

  {#if sources.length}
    <div class="source-list">
      {#each sources as source (source.source_id)}
        <article class:connected={source.runtime.state === 'connected'} class:faulted={source.runtime.state === 'faulted'}>
          <div class="source-summary"><span class="state-dot"></span><div><strong>{source.display_name}</strong><small>{testedIdentity[source.source_id] ?? (isYouTube(source) ? 'YouTube Live' : 'Twitch EventSub')}</small></div><span class="state-label">{stateLabel(source.runtime.state)}</span></div>
          <dl><div><dt>Messages</dt><dd>{source.runtime.messages_received}</dd></div><div><dt>Acceptés</dt><dd>{source.runtime.accepted}</dd></div><div><dt>Session</dt><dd>{source.runtime.session_id?.slice(0, 8) ?? '—'}</dd></div></dl>
          {#if isYouTube(source)}<p class="source-video">Live : <strong>{source.settings.video_id || 'à sélectionner'}</strong></p>{/if}
          {#if source.runtime.fault}<p class="source-detail">Code : {source.runtime.fault.code} · {source.runtime.fault.retryable ? 'nouvel essai possible' : 'action opérateur requise'}</p>{:else if source.runtime.detail}<p class="source-detail">État : {source.runtime.detail}</p>{/if}
          <div class="source-actions">
            {#if !source.authenticated}<button onclick={() => beginAuthorization(source)} disabled={!!busy}><ShieldCheck size={15} /> Autoriser</button>
            {:else}<button onclick={() => testSource(source)} disabled={!!busy || source.desired_state === 'active'}><RefreshCw size={15} /> Tester</button>{#if isYouTube(source) && source.desired_state !== 'active'}<button onclick={() => discoverBroadcasts(source)} disabled={!!busy}><ListVideo size={15} /> Trouver mes lives</button>{/if}{#if source.desired_state === 'active'}<button class="pause" onclick={() => stopSource(source)} disabled={!!busy}><CirclePause size={15} /> Pause</button>{:else}<button class="start" onclick={() => startSource(source)} disabled={!!busy || (isYouTube(source) && !source.settings.video_id)}><Radio size={15} /> Écouter</button>{/if}{/if}
            <button class="delete" aria-label={`Supprimer ${source.display_name}`} onclick={() => deleteSource(source)} disabled={!!busy || source.desired_state === 'active'}><Trash2 size={15} /></button>
          </div>
          {#if broadcastsBySource[source.source_id]?.length}
            <div class="broadcast-list">
              {#each broadcastsBySource[source.source_id] as broadcast (broadcast.video_id)}
                <button onclick={() => selectBroadcast(source, broadcast)} disabled={!!busy}>
                  <span><strong>{broadcast.title}</strong><small>{broadcast.video_id}</small></span><Check size={15} />
                </button>
              {/each}
            </div>
          {/if}
        </article>
      {/each}
    </div>
  {:else}<div class="source-empty"><Radio size={23} /><span><strong>Aucune source configurée</strong><small>L’application reste entièrement utilisable en saisie manuelle.</small></span></div>{/if}
  {#if notice}<p class="source-notice" aria-live="polite"><Check size={15} /> {notice}</p>{/if}
  {#if error}<p class="source-error" role="alert"><TriangleAlert size={16} /> {error}</p>{/if}
</section>

<style>
  .source-panel{margin-top:20px;border:1px solid var(--line);border-radius:18px;background:color-mix(in srgb,var(--surface) 94%,#9146ff 6%);overflow:hidden}.source-heading{display:flex;justify-content:space-between;gap:24px;padding:24px;border-bottom:1px solid var(--line)}.source-title{display:flex;gap:14px;align-items:flex-start}.source-title h2{margin:2px 0 6px;font-size:1.2rem}.source-title p:last-child{margin:0;color:var(--muted);max-width:760px}.source-icon{display:grid;place-items:center;width:40px;height:40px;border-radius:12px;background:#9146ff1c;color:#a970ff}.privacy{display:flex;align-items:center;gap:7px;color:var(--muted);font-size:.78rem;white-space:nowrap}.source-create{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px;align-items:end;padding:18px 24px;border-bottom:1px solid var(--line)}label{display:grid;gap:7px;color:var(--muted);font-size:.76rem;font-weight:650}input,select{min-width:0;border:1px solid var(--line);border-radius:10px;background:var(--background);color:var(--text);padding:11px 12px}.policy{grid-template-columns:auto 1fr;align-items:start;line-height:1.4}.policy input{width:17px;height:17px;padding:0}button,a{display:inline-flex;align-items:center;justify-content:center;gap:7px;min-height:40px;border-radius:10px;border:1px solid var(--line);background:var(--surface);color:var(--text);padding:9px 13px;font:inherit;font-size:.8rem;font-weight:700;text-decoration:none;cursor:pointer}button:disabled{opacity:.48;cursor:not-allowed}.source-create>button,.start{border-color:#9146ff66;background:#9146ff;color:#fff}.authorization{display:flex;justify-content:space-between;gap:20px;padding:18px 24px;background:#9146ff12;border-bottom:1px solid #9146ff35}.authorization>div:first-child{display:grid;gap:2px}.authorization span,.authorization small{color:var(--muted);font-size:.72rem}.authorization strong{font:750 1.45rem/1.2 ui-monospace,monospace;letter-spacing:.12em}.authorization-actions{display:flex;align-items:center;flex-wrap:wrap;gap:8px}.authorization-actions a{background:#9146ff;border-color:#9146ff;color:#fff}.source-list{display:grid;grid-template-columns:repeat(auto-fit,minmax(290px,1fr));gap:12px;padding:18px 24px}article{border:1px solid var(--line);border-radius:14px;background:var(--background);padding:16px}article.connected{border-color:#39c98166}article.faulted{border-color:#f06d6d70}.source-summary{display:grid;grid-template-columns:auto 1fr auto;align-items:center;gap:10px}.source-summary>div{display:grid}.source-summary small{color:var(--muted);font-size:.72rem}.state-dot{width:9px;height:9px;border-radius:50%;background:#8c929b;box-shadow:0 0 0 4px #8c929b20}article.connected .state-dot{background:#39c981;box-shadow:0 0 0 4px #39c98120}article.faulted .state-dot{background:#f06d6d;box-shadow:0 0 0 4px #f06d6d20}.state-label{color:var(--muted);font-size:.72rem}dl{display:grid;grid-template-columns:repeat(3,1fr);margin:14px 0;border-block:1px solid var(--line)}dl div{padding:10px 4px}dt{color:var(--muted);font-size:.66rem}dd{margin:3px 0 0;font:700 .82rem ui-monospace,monospace}.source-detail{margin:-4px 0 12px;color:#e48d8d;font:.7rem ui-monospace,monospace}.source-actions{display:flex;flex-wrap:wrap;gap:8px}.source-actions .delete{margin-left:auto;color:#e48d8d;padding-inline:11px}.source-actions .pause{color:#e6b76e}.source-empty{display:flex;align-items:center;justify-content:center;gap:12px;padding:26px;color:var(--muted)}.source-empty span{display:grid}.source-empty strong{color:var(--text)}.source-empty small{margin-top:2px}.source-notice,.source-error{display:flex;align-items:center;gap:8px;margin:0;padding:12px 24px;border-top:1px solid var(--line);font-size:.8rem}.source-notice{color:#55d99a}.source-error{color:#ef8f8f}@media(max-width:760px){.source-heading,.authorization{flex-direction:column}.privacy{white-space:normal}.source-create{grid-template-columns:1fr}.source-list{grid-template-columns:1fr}}
  .source-video{margin:-4px 0 12px;color:var(--muted);font-size:.72rem}.source-video strong{color:var(--text);font-family:ui-monospace,monospace}.broadcast-list{display:grid;gap:7px;margin-top:12px;padding-top:12px;border-top:1px solid var(--line)}.broadcast-list button{justify-content:space-between;text-align:left}.broadcast-list span{display:grid;min-width:0}.broadcast-list strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.broadcast-list small{color:var(--muted);font-family:ui-monospace,monospace}
</style>
