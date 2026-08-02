<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { Brain, RefreshCw, RotateCcw, TriangleAlert } from '@lucide/svelte';
  import type { MemoryEntry } from './contracts';

  let {
    inTauri,
    contextSha256,
    revision,
  }: {
    inTauri: boolean;
    contextSha256: string | null;
    revision: number;
  } = $props();

  let entries = $state<MemoryEntry[]>([]);
  let busy = $state(false);
  let error = $state('');
  let loadedContext = '';
  let loadedRevision = -1;
  let loadGeneration = 0;

  $effect(() => {
    revision;
    const context = contextSha256;
    if (inTauri && context && (context !== loadedContext || revision !== loadedRevision)) {
      loadedContext = context;
      loadedRevision = revision;
      entries = [];
      void load(context);
    } else if (!context) {
      loadGeneration += 1;
      busy = false;
      loadedContext = '';
      loadedRevision = -1;
      entries = [];
    }
  });

  async function load(context = contextSha256) {
    if (!inTauri || !context) return;
    const generation = ++loadGeneration;
    busy = true;
    error = '';
    try {
      const loaded = await invoke<MemoryEntry[]>('list_memory_ipc', {
        contextPackageSha256: context,
        limit: 1000,
        activeOnly: true,
      });
      if (generation === loadGeneration && context === contextSha256) entries = loaded;
    } catch (cause) {
      if (generation === loadGeneration) {
        error = `Mémoire indisponible : ${cause instanceof Error ? cause.message : String(cause)}`;
      }
    } finally {
      if (generation === loadGeneration) busy = false;
    }
  }

  async function revoke(entry: MemoryEntry) {
    if (entry.state !== 'active' || busy) return;
    const approved = await confirm(
      `Révoquer « ${entry.expression} » pour ${entry.target_id} ? L’effet sera immédiat et l’historique technique restera borné.`,
      { title: 'Révoquer une formulation apprise', kind: 'warning' },
    );
    if (!approved) return;
    busy = true;
    error = '';
    try {
      const revoked = await invoke<MemoryEntry>('revoke_memory_ipc', {
        contextPackageSha256: entry.context_package_sha256,
        id: entry.id,
      });
      entries = entries.map((candidate) => candidate.id === revoked.id ? revoked : candidate);
    } catch (cause) {
      error = `Révocation refusée : ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      busy = false;
    }
  }

  function stateLabel(state: MemoryEntry['state']) {
    return { active: 'active', revoked: 'révoquée', expired: 'expirée', evicted: 'évincée' }[state];
  }
</script>

<section class="memory-panel" aria-labelledby="memory-title">
  <div class="heading">
    <div class="title">
      <Brain size={19} />
      <div>
        <h2 id="memory-title">Mémoire de reconnaissance</h2>
        <p>Formulations apprises après arbitrage explicite · contexte actif uniquement</p>
      </div>
    </div>
    <button class="refresh" onclick={() => load()} disabled={!contextSha256 || busy} aria-label="Actualiser la mémoire">
      <RefreshCw size={15} class={busy ? 'spinning' : ''} /> Actualiser
    </button>
  </div>

  {#if !contextSha256}
    <p class="empty">Activez un paquet de contexte pour consulter et tuner sa mémoire.</p>
  {:else if entries.length}
    <div class="memory-list">
      {#each entries as entry (entry.id)}
        <article class:inactive={entry.state !== 'active'}>
          <div class="expression">
            <strong>{entry.expression}</strong>
            <code>{entry.target_id}</code>
          </div>
          <dl>
            <div><dt>État</dt><dd>{stateLabel(entry.state)}</dd></div>
            <div><dt>Utilisations</dt><dd>{entry.use_count}</dd></div>
            <div><dt>Expiration</dt><dd>{new Date(entry.expires_at_ms).toLocaleDateString('fr-FR')}</dd></div>
          </dl>
          <button class="revoke" onclick={() => revoke(entry)} disabled={entry.state !== 'active' || busy}>
            <RotateCcw size={14} /> Révoquer
          </button>
        </article>
      {/each}
    </div>
  {:else}
    <p class="empty">Aucune formulation apprise pour cette version du contexte.</p>
  {/if}

  {#if error}<p class="error" role="alert"><TriangleAlert size={15} /> {error}</p>{/if}
  <p class="privacy">Le texte n’est conservé que lorsque vous cochez explicitement l’apprentissage. Quota : 1 000 entrées actives · TTL : 30 jours · historique borné.</p>
</section>

<style>
  .memory-panel { margin-top: 18px; border: 1px solid #2e332b; border-radius: 6px; padding: 20px; background: #171a15; }
  .heading, .title { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .title { justify-content: flex-start; color: #c8f15b; }
  h2 { margin: 0; color: #ecece4; font-size: 14px; }
  .title p { margin: 3px 0 0; color: #7f877a; font-size: 10px; }
  button { min-height: 38px; display: inline-flex; align-items: center; justify-content: center; gap: 7px; border-radius: 4px; cursor: pointer; font-size: 10px; font-weight: 800; }
  button:disabled { cursor: not-allowed; opacity: .42; }
  .refresh { border: 1px solid #3b4336; padding: 0 12px; color: #cbd1c5; background: #20251d; }
  .memory-list { display: grid; gap: 7px; margin-top: 16px; }
  article { display: grid; grid-template-columns: minmax(170px, 1fr) minmax(260px, 1.2fr) auto; gap: 16px; align-items: center; padding: 12px; border: 1px solid #343a30; background: #1c2019; }
  article.inactive { opacity: .62; }
  .expression { min-width: 0; }
  .expression strong { display: block; overflow-wrap: anywhere; color: #e6e8df; font-size: 12px; }
  .expression code { display: block; margin-top: 4px; color: #89947d; font-size: 9px; }
  dl { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; margin: 0; }
  dl div { min-width: 0; }
  dt { color: #737b6f; font-size: 8px; text-transform: uppercase; letter-spacing: .06em; }
  dd { margin: 3px 0 0; color: #bdc4b7; font-size: 10px; }
  .revoke { border: 1px solid #5a4432; padding: 0 11px; color: #efbd7f; background: #261f17; }
  .empty { margin: 16px 0 0; padding: 14px; border: 1px dashed #343a30; color: #81897d; font-size: 10px; text-align: center; }
  .error { display: flex; gap: 7px; align-items: center; margin: 12px 0 0; color: #ef9b93; font-size: 10px; }
  .privacy { margin: 13px 0 0; color: #697064; font-size: 9px; line-height: 1.5; }
  :global(.spinning) { animation: spin .8s linear infinite; }
  button:focus-visible { outline: 2px solid #c8f15b; outline-offset: 2px; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (max-width: 820px) {
    .heading { align-items: flex-start; }
    article { grid-template-columns: 1fr; gap: 10px; }
    dl { grid-template-columns: repeat(3, minmax(0, 1fr)); }
    .revoke { justify-self: start; }
  }
</style>
