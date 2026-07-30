<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import {
    Check,
    Library,
    RotateCcw,
    Search,
    SlidersHorizontal,
    TriangleAlert,
  } from '@lucide/svelte';
  import type { ContextPackagePreview, TargetRecord } from './contracts';

  let {
    activeContext,
    onUseTarget,
  }: {
    activeContext: ContextPackagePreview | null;
    onUseTarget: (target: TargetRecord) => void;
  } = $props();

  let query = $state('');
  let records = $state<TargetRecord[]>([]);
  let selected = $state<TargetRecord | null>(null);
  let canonical = $state('');
  let aliases = $state('');
  let operation = $state<'idle' | 'search' | 'save' | 'discard'>('idle');
  let error = $state('');
  let observedPackage = '';

  $effect(() => {
    const packageSha256 = activeContext?.package_sha256 ?? '';
    if (packageSha256 !== observedPackage) {
      observedPackage = packageSha256;
      query = '';
      records = [];
      select(null);
      if (packageSha256) void searchTargets();
    }
  });

  function select(record: TargetRecord | null) {
    selected = record;
    canonical = record?.canonical ?? '';
    aliases = record?.aliases.join('\n') ?? '';
  }

  async function searchTargets() {
    if (!activeContext || operation !== 'idle') return;
    operation = 'search';
    error = '';
    try {
      records = await invoke<TargetRecord[]>('find_targets_ipc', { query: query.trim() });
      if (selected) {
        select(records.find((record) => record.id === selected?.id) ?? null);
      }
    } catch (cause) {
      error = `Recherche impossible : ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      operation = 'idle';
    }
  }

  async function saveDraft() {
    if (!selected || operation !== 'idle') return;
    operation = 'save';
    error = '';
    try {
      const saved = await invoke<TargetRecord>('save_target_draft_ipc', {
        packageSha256: selected.package_sha256,
        target: {
          id: selected.id,
          canonical: canonical.trim(),
          aliases: aliases
            .split('\n')
            .map((alias) => alias.trim())
            .filter(Boolean),
        },
      });
      records = records.map((record) => (record.id === saved.id ? saved : record));
      select(saved);
    } catch (cause) {
      error = `Brouillon refusé : ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      operation = 'idle';
    }
  }

  async function discardDraft() {
    if (!selected?.is_draft || operation !== 'idle') return;
    operation = 'discard';
    error = '';
    const targetId = selected.id;
    try {
      await invoke<boolean>('discard_target_draft_ipc', {
        packageSha256: selected.package_sha256,
        targetId,
      });
      records = await invoke<TargetRecord[]>('find_targets_ipc', { query: query.trim() });
      select(records.find((record) => record.id === targetId) ?? null);
    } catch (cause) {
      error = `Restauration impossible : ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      operation = 'idle';
    }
  }
</script>

<section class="workshop" aria-labelledby="workshop-title">
  <header>
    <div class="title">
      <span class="icon"><Library size={19} /></span>
      <div>
        <p>Atelier du contexte</p>
        <h2 id="workshop-title">Voir et régler le dictionnaire</h2>
        <small>Le paquet publié reste intact. Les modifications sont des brouillons locaux réversibles.</small>
      </div>
    </div>
    {#if activeContext}
      <span class="scope">{activeContext.target_count} cibles · v{activeContext.version}</span>
    {/if}
  </header>

  {#if activeContext}
    <div class="body">
      <div class="catalogue">
        <form onsubmit={(event) => { event.preventDefault(); void searchTargets(); }}>
          <label for="target-search">Rechercher un titre ou un alias</label>
          <div class="search">
            <Search class="search-icon" size={15} />
            <input id="target-search" bind:value={query} maxlength="256" placeholder="Witcher, Chihiro, ER…" />
            <button type="submit" disabled={operation !== 'idle'}>
              {operation === 'search' ? 'Recherche…' : 'Chercher'}
            </button>
          </div>
        </form>

        <div class="results" aria-live="polite">
          {#if records.length}
            {#each records as record}
              <button
                class:selected={selected?.id === record.id}
                type="button"
                onclick={() => select(record)}
              >
                <span>
                  <strong>{record.canonical}</strong>
                  <small>{record.aliases.slice(0, 3).join(' · ') || 'Aucun alias'}</small>
                </span>
                {#if record.is_draft}<em>Brouillon</em>{/if}
              </button>
            {/each}
          {:else if operation === 'search'}
            <p>Recherche du contexte actif…</p>
          {:else}
            <p>Aucune cible ne correspond. Essayez le titre canonique ou un alias.</p>
          {/if}
        </div>
      </div>

      <div class="editor">
        {#if selected}
          <div class="editor-heading">
            <SlidersHorizontal size={17} />
            <div>
              <span>{selected.is_draft ? 'Brouillon local' : 'Version publiée'}</span>
              <code>{selected.id}</code>
            </div>
          </div>

          <label for="draft-canonical">Titre canonique</label>
          <input id="draft-canonical" bind:value={canonical} maxlength="256" />

          <label for="draft-aliases">Alias <span>un par ligne · 64 maximum</span></label>
          <textarea id="draft-aliases" bind:value={aliases} rows="6" maxlength="16448"></textarea>

          <div class="editor-actions">
            <button class="save" type="button" onclick={saveDraft} disabled={operation !== 'idle' || !canonical.trim()}>
              <Check size={15} /> {operation === 'save' ? 'Enregistrement…' : 'Enregistrer localement'}
            </button>
            <button class="use" type="button" onclick={() => onUseTarget(selected!)} disabled={operation !== 'idle'}>
              Utiliser pour la manche
            </button>
            <button class="discard" type="button" onclick={discardDraft} disabled={!selected.is_draft || operation !== 'idle'}>
              <RotateCcw size={14} /> Revenir au publié
            </button>
          </div>
        {:else}
          <div class="editor-empty">
            <SlidersHorizontal size={27} />
            <strong>Sélectionnez une cible</strong>
            <p>Vous pourrez corriger son titre, ajouter des initiales ou une variante entendue en live.</p>
          </div>
        {/if}
      </div>
    </div>
  {:else}
    <div class="empty">
      <Library size={26} />
      <div><strong>Aucun contexte actif</strong><p>Activez un paquet pour parcourir ses cibles et créer des réglages locaux.</p></div>
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert"><TriangleAlert class="error-icon" size={16} /> {error}</p>
  {/if}
</section>

<style>
  .workshop { padding: 25px clamp(24px, 4vw, 58px) 28px; border-bottom: 1px solid #30342d; background: #161914; }
  header, .title, .editor-heading, .empty { display: flex; align-items: flex-start; }
  header { justify-content: space-between; gap: 24px; }
  .title { gap: 12px; min-width: 0; }
  .icon { width: 37px; height: 37px; display: grid; place-items: center; flex: 0 0 auto; color: #c8f15b; background: #26301d; border: 1px solid #425231; }
  .title p { margin: 0 0 3px; color: #92988c; font-size: 9px; text-transform: uppercase; letter-spacing: .09em; }
  h2 { margin: 0; color: #e9e9e1; font-size: 16px; }
  .title small { display: block; margin-top: 5px; color: #7f877a; font-size: 10px; line-height: 1.45; }
  .scope { flex: 0 0 auto; padding: 6px 8px; border: 1px solid #394033; color: #aab2a3; font: 600 9px/1.2 "Cascadia Code", Consolas, monospace; }
  .body { display: grid; grid-template-columns: minmax(300px, .9fr) minmax(360px, 1.1fr); gap: 26px; margin-top: 23px; }
  label { display: flex; justify-content: space-between; gap: 12px; margin: 0 0 7px; color: #c8cec1; font-size: 10px; font-weight: 700; }
  label span { color: #73796e; font-weight: 500; }
  .search { display: grid; grid-template-columns: auto 1fr auto; align-items: center; border: 1px solid #373c33; background: #11130f; }
  :global(.search-icon) { margin-left: 11px; color: #7e8678; }
  input, textarea { min-width: 0; border: 1px solid #373c33; border-radius: 4px; padding: 10px 11px; color: #e9e9e1; background: #11130f; }
  .search input { border: 0; border-radius: 0; }
  input:focus, textarea:focus { border-color: #6c7759; outline: none; }
  .search input:focus { box-shadow: inset 0 0 0 1px #6c7759; }
  .search button { align-self: stretch; border: 0; border-left: 1px solid #373c33; padding: 0 13px; color: #c8f15b; background: #1e2419; cursor: pointer; font-size: 10px; font-weight: 700; }
  .results { max-height: 292px; margin-top: 10px; overflow: auto; border-top: 1px solid #30342d; }
  .results > button { width: 100%; min-height: 54px; display: flex; align-items: center; justify-content: space-between; gap: 12px; border: 0; border-bottom: 1px solid #292d27; padding: 9px 10px; color: #c5cbbf; background: transparent; cursor: pointer; text-align: left; }
  .results > button:hover, .results > button.selected { background: #20241e; }
  .results > button.selected { box-shadow: inset 2px 0 #c8f15b; }
  .results strong, .results small { display: block; overflow-wrap: anywhere; }
  .results strong { font-size: 11px; }
  .results small { margin-top: 4px; color: #777f73; font-size: 9px; }
  .results em { flex: 0 0 auto; color: #f1bd5b; font-size: 8px; font-style: normal; text-transform: uppercase; letter-spacing: .08em; }
  .results > p { margin: 18px 10px; color: #737a6e; font-size: 10px; line-height: 1.5; }
  .editor { min-width: 0; min-height: 344px; padding-left: 26px; border-left: 1px solid #30342d; }
  .editor-heading { gap: 9px; margin-bottom: 17px; color: #c8f15b; }
  .editor-heading span, .editor-heading code { display: block; }
  .editor-heading span { color: #a7aea1; font-size: 10px; font-weight: 700; }
  .editor-heading code { margin-top: 3px; color: #727a6e; font-size: 9px; overflow-wrap: anywhere; }
  .editor > input, .editor > textarea { width: 100%; }
  .editor > input { margin-bottom: 15px; }
  textarea { resize: vertical; min-height: 119px; line-height: 1.45; }
  .editor-actions { display: grid; grid-template-columns: 1.1fr 1fr; gap: 8px; margin-top: 10px; }
  .editor-actions button { min-height: 40px; display: flex; align-items: center; justify-content: center; gap: 7px; border-radius: 4px; cursor: pointer; font-size: 9px; font-weight: 800; }
  .save { border: 1px solid #7c943e; color: #171a12; background: #c8f15b; }
  .use { border: 1px solid #4e5946; color: #d7ddd0; background: #272c23; }
  .discard { grid-column: 1 / -1; border: 1px solid #42473e; color: #969d90; background: transparent; }
  button:disabled { cursor: not-allowed; opacity: .42; }
  .editor-empty { min-height: 280px; display: grid; place-items: center; align-content: center; text-align: center; color: #5f665b; }
  .editor-empty strong { margin-top: 12px; color: #a5aca0; font-size: 11px; }
  .editor-empty p { max-width: 285px; margin: 6px 0 0; color: #777e72; font-size: 10px; line-height: 1.5; }
  .empty { gap: 12px; margin-top: 22px; padding: 18px; border: 1px dashed #394036; color: #687063; }
  .empty strong { color: #a4ab9f; font-size: 11px; }
  .empty p { margin: 4px 0 0; font-size: 10px; }
  .error { display: flex; gap: 8px; margin: 15px 0 0; color: #ef9b93; font-size: 10px; }
  :global(.error-icon) { flex: 0 0 auto; }
  button:focus-visible, input:focus-visible, textarea:focus-visible { outline: 2px solid #c8f15b; outline-offset: 2px; }
  @media (max-width: 800px) {
    .body { grid-template-columns: 1fr; }
    .editor { padding: 22px 0 0; border-left: 0; border-top: 1px solid #30342d; }
  }
  @media (max-width: 600px) {
    .workshop { padding-inline: 20px; }
    header { display: block; }
    .scope { display: inline-block; margin-top: 13px; }
    .editor-actions { grid-template-columns: 1fr; }
    .discard { grid-column: auto; }
  }
</style>
