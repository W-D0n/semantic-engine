<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { CircleDot, Copy, Eye, EyeOff, Globe2, Power, ShieldCheck, TriangleAlert } from '@lucide/svelte';
  import type { LoopbackStatus } from './contracts';

  let { inTauri }: { inTauri: boolean } = $props();
  let status = $state<LoopbackStatus>({
    running: false,
    address: null,
    token: null,
    protocol_version: 1,
    allowed_origins: [],
  });
  let port = $state(17831);
  let origin = $state('http://localhost:5173');
  let busy = $state(false);
  let error = $state('');
  let revealToken = $state(false);
  let copied = $state(false);

  onMount(() => {
    if (inTauri) void refresh();
  });

  async function refresh() {
    try {
      status = await invoke<LoopbackStatus>('loopback_status_ipc');
    } catch (cause) {
      error = message(cause);
    }
  }

  async function start() {
    if (!inTauri || busy) return;
    busy = true;
    error = '';
    copied = false;
    try {
      status = await invoke<LoopbackStatus>('start_loopback_ipc', {
        port,
        origin: origin.trim() || null,
      });
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }

  async function stop() {
    if (!inTauri || busy || !status.running) return;
    busy = true;
    error = '';
    try {
      status = await invoke<LoopbackStatus>('stop_loopback_ipc');
      revealToken = false;
      copied = false;
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }

  async function copyToken() {
    if (!status.token) return;
    try {
      await navigator.clipboard.writeText(status.token);
      copied = true;
    } catch (cause) {
      error = `Copie impossible : ${message(cause)}`;
    }
  }

  function message(cause: unknown) {
    return cause instanceof Error ? cause.message : String(cause);
  }
</script>

<section class:running={status.running} class="loopback-panel" aria-labelledby="loopback-heading">
  <div class="loopback-intro">
    <span class="loopback-icon"><Globe2 size={20} /></span>
    <div>
      <p class="eyebrow">Intégration locale optionnelle</p>
      <h2 id="loopback-heading">API HTTP et WebSocket</h2>
      <p>
        Désactivée au démarrage. Elle écoute uniquement sur cet appareil et partage la session
        du moteur avec l’interface, sans dépendre d’une webapp externe.
      </p>
    </div>
  </div>

  <div class="loopback-state" aria-live="polite">
    <CircleDot size={15} />
    <span>
      <strong>{status.running ? 'Passerelle active' : 'Aucun port réseau ouvert'}</strong>
      <small>{status.address ?? 'Activation manuelle requise'}</small>
    </span>
  </div>

  {#if !status.running}
    <div class="loopback-form">
      <label for="loopback-port">Port local
        <input id="loopback-port" type="number" min="0" max="65535" bind:value={port} />
      </label>
      <label for="loopback-origin">Origine web autorisée
        <input id="loopback-origin" bind:value={origin} maxlength="512" placeholder="http://localhost:5173" />
      </label>
      <button class="start" onclick={start} disabled={!inTauri || busy}>
        <Power size={16} /> {busy ? 'Ouverture…' : 'Activer explicitement'}
      </button>
    </div>
  {:else}
    <div class="loopback-active">
      <div class="secret-field">
        <label for="loopback-token">Jeton éphémère</label>
        <div>
          <input id="loopback-token" readonly type={revealToken ? 'text' : 'password'} value={status.token ?? ''} />
          <button aria-label={revealToken ? 'Masquer le jeton' : 'Afficher le jeton'} title={revealToken ? 'Masquer' : 'Afficher'} onclick={() => (revealToken = !revealToken)}>
            {#if revealToken}<EyeOff size={16} />{:else}<Eye size={16} />{/if}
          </button>
          <button aria-label="Copier le jeton" title="Copier le jeton" onclick={copyToken}>
            <Copy size={16} />
          </button>
        </div>
        <small>{copied ? 'Jeton copié.' : 'À transmettre au client local sans le placer dans une URL ou un log.'}</small>
      </div>
      <div class="active-actions">
        <p><ShieldCheck size={15} /> v{status.protocol_version} · {status.allowed_origins.length || 0} origine autorisée</p>
        <button class="stop" onclick={stop} disabled={busy}><Power size={16} /> {busy ? 'Fermeture…' : 'Désactiver'}</button>
      </div>
    </div>
  {/if}

  {#if error}
    <p class="loopback-error" role="alert"><TriangleAlert size={16} /> {error}</p>
  {/if}
</section>

<style>
  .loopback-panel { display: grid; grid-template-columns: minmax(300px, 1fr) auto; align-items: center; gap: 20px 30px; padding: 24px clamp(24px, 4vw, 58px); border-top: 1px solid var(--line); border-bottom: 1px solid var(--line); background: #151813; }
  .loopback-panel.running { background: #171c14; }
  .loopback-intro { display: flex; align-items: flex-start; gap: 14px; min-width: 0; }
  .loopback-icon { width: 40px; height: 40px; flex: 0 0 auto; display: grid; place-items: center; color: var(--lime); border: 1px solid #3a422e; background: #1b2115; }
  .loopback-intro .eyebrow { margin-bottom: 6px; }
  h2 { margin: 0; color: var(--ink); font-size: 15px; }
  .loopback-intro > div > p:last-child { max-width: 700px; margin: 5px 0 0; color: var(--muted); font-size: 11px; line-height: 1.5; }
  .loopback-state { min-width: 210px; display: flex; align-items: center; gap: 9px; color: #737b6d; }
  .running .loopback-state { color: var(--lime); }
  .loopback-state span, .loopback-state strong, .loopback-state small { display: block; }
  .loopback-state strong { color: #c9cfc2; font-size: 10px; }
  .loopback-state small { margin-top: 3px; color: #777f72; font: 500 9px/1.3 "Cascadia Code", Consolas, monospace; }
  .loopback-form, .loopback-active { grid-column: 1 / -1; display: grid; grid-template-columns: 130px minmax(260px, 1fr) auto; align-items: end; gap: 12px; padding-top: 18px; border-top: 1px solid #2b3028; }
  .loopback-form label, .secret-field > label { display: grid; gap: 7px; margin: 0; color: #aeb5a8; font-size: 10px; }
  .loopback-form input, .secret-field input { min-height: 41px; padding: 9px 10px; font: 500 10px/1.2 "Cascadia Code", Consolas, monospace; }
  button { min-height: 41px; display: flex; align-items: center; justify-content: center; gap: 7px; border-radius: 4px; padding: 9px 13px; cursor: pointer; font-size: 10px; font-weight: 800; }
  button:disabled { opacity: .42; cursor: not-allowed; }
  .start { color: #161910; border: 1px solid var(--lime); background: var(--lime); }
  .start:hover { background: #d5fa72; }
  .loopback-active { grid-template-columns: minmax(300px, 1fr) auto; }
  .secret-field > div { display: grid; grid-template-columns: 1fr 42px 42px; }
  .secret-field input { border-radius: 4px 0 0 4px; }
  .secret-field div button { min-width: 42px; border: 1px solid #3b4137; border-left: 0; border-radius: 0; padding: 0; color: #aeb5a7; background: #1b1f19; }
  .secret-field div button:last-child { border-radius: 0 4px 4px 0; }
  .secret-field small { display: block; margin-top: 6px; color: #747c70; font-size: 9px; }
  .active-actions { display: flex; align-items: end; gap: 12px; }
  .active-actions p { display: flex; align-items: center; gap: 6px; margin: 0 0 12px; color: #9ca493; font-size: 9px; }
  .stop { color: #ef9a91; border: 1px solid #5a3834; background: #241917; }
  .stop:hover { border-color: #8d4e48; background: #2c1d1a; }
  .loopback-error { grid-column: 1 / -1; display: flex; align-items: center; gap: 8px; margin: 0; color: #ef9a91; font-size: 10px; }
  @media (max-width: 760px) {
    .loopback-panel { display: block; padding: 22px 20px; }
    .loopback-state { margin-top: 18px; }
    .loopback-form, .loopback-active { display: grid; grid-template-columns: 1fr; margin-top: 18px; }
    .active-actions { align-items: stretch; flex-direction: column; }
    .active-actions p { margin-bottom: 0; }
  }
</style>
