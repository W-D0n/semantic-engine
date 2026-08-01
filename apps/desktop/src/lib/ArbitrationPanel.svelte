<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { Check, Gavel, TriangleAlert, X } from '@lucide/svelte';
  import type { HistoryItem, OperatorResolution, Round } from './contracts';

  let {
    result,
    round,
    sessionId,
    onResolved,
  }: {
    result: HistoryItem;
    round: Round | null;
    sessionId: string;
    onResolved: (resolution: OperatorResolution) => void;
  } = $props();

  let note = $state('');
  let busy = $state(false);
  let error = $state('');
  let currentMessageId = '';

  $effect(() => {
    if (result.message_id !== currentMessageId) {
      currentMessageId = result.message_id;
      note = result.resolution?.note ?? '';
      error = '';
    }
  });

  async function resolve(verdict: 'accepted' | 'rejected') {
    if (!round || busy) return;
    busy = true;
    error = '';
    try {
      const resolution = await invoke<OperatorResolution>('resolve_session_ipc', {
        sessionId,
        request: {
          round_id: result.round_id,
          message_id: result.message_id,
          verdict,
          target_id: verdict === 'accepted' ? round.targets[0]?.id ?? null : null,
          note: note.trim(),
        },
      });
      onResolved(resolution);
    } catch (cause) {
      error = `Arbitrage refusé : ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      busy = false;
    }
  }
</script>

<section class="arbitration" aria-labelledby="arbitration-title">
  <div class="heading">
    <Gavel class="heading-icon" size={17} />
    <div>
      <h3 id="arbitration-title">Arbitrage manuel</h3>
      <p>La décision moteur reste dans la trace ; votre correction devient le signal final.</p>
    </div>
  </div>

  {#if result.resolution}
    <div class="resolved" aria-live="polite">
      <span>Décision opérateur</span>
      <strong>{result.resolution.final_decision === 'accepted' ? 'Acceptée' : 'Rejetée'}</strong>
      {#if result.resolution.note}<p>{result.resolution.note}</p>{/if}
    </div>
  {/if}

  <label for="resolution-note">Note d’arbitrage <span>optionnelle · 512 caractères</span></label>
  <input
    id="resolution-note"
    bind:value={note}
    maxlength="512"
    placeholder="Ex. faute évidente, réponse prononcée à l’oral…"
  />

  <div class="actions">
    <button class="accept" onclick={() => resolve('accepted')} disabled={!round || busy}>
      <Check size={15} /> {busy ? 'Traitement…' : 'Accepter'}
    </button>
    <button class="reject" onclick={() => resolve('rejected')} disabled={!round || busy}>
      <X size={15} /> Rejeter
    </button>
  </div>

  {#if error}
    <p class="error" role="alert"><TriangleAlert class="error-icon" size={15} /> {error}</p>
  {/if}
</section>

<style>
  .arbitration { margin-top: 22px; padding-top: 20px; border-top: 1px solid #30342d; }
  .heading { display: flex; gap: 9px; align-items: flex-start; }
  :global(.heading-icon) { flex: 0 0 auto; margin-top: 1px; color: #f1bd5b; }
  h3 { margin: 0; color: #e9e9e1; font-size: 12px; }
  .heading p { margin: 4px 0 0; color: #858c80; font-size: 10px; line-height: 1.5; }
  label { display: flex; justify-content: space-between; gap: 12px; margin: 15px 0 7px; color: #c8cec1; font-size: 10px; font-weight: 700; }
  label span { color: #73796e; font-weight: 500; text-align: right; }
  input { width: 100%; min-height: 40px; border: 1px solid #373c33; border-radius: 4px; padding: 9px 10px; color: #e9e9e1; background: #131510; }
  input:focus { border-color: #6c7759; outline: 2px solid transparent; }
  .actions { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 9px; }
  button { min-height: 40px; display: flex; align-items: center; justify-content: center; gap: 7px; border-radius: 4px; cursor: pointer; font-size: 10px; font-weight: 800; }
  button:disabled { cursor: not-allowed; opacity: .45; }
  .accept { border: 1px solid #738c38; color: #171a12; background: #c8f15b; }
  .accept:hover:not(:disabled) { background: #d5fa72; }
  .reject { border: 1px solid #5a3531; color: #ef9b93; background: #241816; }
  .reject:hover:not(:disabled) { border-color: #8d4d47; color: #ffb0a8; }
  .resolved { margin-top: 14px; padding: 10px 11px; border: 1px solid #475337; background: #1f2619; }
  .resolved span { color: #8e9588; font-size: 9px; text-transform: uppercase; letter-spacing: .07em; }
  .resolved strong { display: block; margin-top: 3px; color: #c8f15b; font-size: 12px; }
  .resolved p { margin: 6px 0 0; color: #aeb5a7; font-size: 10px; overflow-wrap: anywhere; }
  .error { display: flex; gap: 7px; margin: 10px 0 0; color: #ef9b93; font-size: 10px; }
  :global(.error-icon) { flex: 0 0 auto; }
  button:focus-visible, input:focus-visible { outline: 2px solid #c8f15b; outline-offset: 2px; }
</style>
