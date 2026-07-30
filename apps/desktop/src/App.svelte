<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import {
    Braces,
    Check,
    CircleDot,
    Database,
    Play,
    Radio,
    RotateCcw,
    ShieldCheck,
    TriangleAlert,
    X,
    Zap,
  } from '@lucide/svelte';

  type Decision = 'accepted' | 'rejected' | 'abstained';

  type Validation = {
    round_id: string;
    message_id: string;
    participant_id: string;
    source_sequence: number;
    decision: Decision;
    target_id: string | null;
    score: number;
    evidence: Array<{
      kind: 'configured_expression' | 'normalized_expression' | 'fuzzy_expression' | 'ambiguous_expression';
      matched_expression: string;
    }>;
    issue?: 'invalid_policy' | 'invalid_round' | 'invalid_submission';
  };

  type HistoryItem = Validation & { input: string; latency: number };

  const samples = [
    { label: 'Exact', value: 'Elden Ring' },
    { label: 'Faute', value: 'eldern ring' },
    { label: 'Limite', value: 'elden kings' },
    { label: 'Hors cible', value: 'dark souls' },
  ];

  let canonical = $state('Elden Ring');
  let aliases = $state('ER');
  let message = $state('eldern ring');
  let participant = $state('viewer-07');
  let sequence = $state(101);
  let busy = $state(false);
  let error = $state('');
  let history = $state<HistoryItem[]>([]);
  let result = $state<HistoryItem | null>(null);

  const inTauri = '__TAURI_INTERNALS__' in window;

  async function runValidation() {
    if (!message.trim() || !canonical.trim() || busy) return;
    if (!inTauri) {
      error = "L’aperçu web affiche l’interface, mais la validation s’exécute dans l’application Tauri.";
      return;
    }

    busy = true;
    error = '';
    const startedAt = performance.now();

    try {
      const validation = await invoke<Validation>('validate', {
        round: {
          id: 'desktop-round',
          targets: [
            {
              id: canonical.toLowerCase().replaceAll(/[^a-z0-9]+/g, '-').replaceAll(/^-|-$/g, ''),
              canonical: canonical.trim(),
              aliases: aliases
                .split('\n')
                .map((alias) => alias.trim())
                .filter(Boolean),
            },
          ],
          policy: { accept_threshold: 0.87, review_threshold: 0.72, ambiguity_margin: 0.05 },
        },
        submission: {
          message_id: `desktop-${sequence}`,
          participant_id: participant.trim() || 'anonymous',
          source_sequence: sequence,
          text: message,
        },
      });
      const item = { ...validation, input: message, latency: performance.now() - startedAt };
      result = item;
      history = [item, ...history].slice(0, 8);
      sequence += 1;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }

  function resetSession() {
    result = null;
    history = [];
    error = '';
    sequence = 101;
  }

  function decisionLabel(decision: Decision) {
    return { accepted: 'Acceptée', abstained: 'À arbitrer', rejected: 'Rejetée' }[decision];
  }
</script>

<svelte:head><title>Semantic Engine — Console locale</title></svelte:head>

<main class="app-shell">
  <header class="topbar">
    <a class="brand" href="#console" aria-label="Semantic Engine, retour à la console">
      <span class="brand-mark"><CircleDot size={18} strokeWidth={2.4} /></span>
      <span>
        <strong>Semantic Engine</strong>
        <small>Console de validation locale</small>
      </span>
    </a>
    <div class="runtime-status" class:offline={!inTauri}>
      <span class="status-dot"></span>
      <span>{inTauri ? 'Moteur Rust prêt' : 'Aperçu interface'}</span>
      <kbd>LOCAL</kbd>
    </div>
  </header>

  <section class="stage" id="console">
    <div class="stage-copy">
      <p class="eyebrow"><Radio size={14} /> Laboratoire de manche</p>
      <h1>Valider le sens, <em>pas la frappe.</em></h1>
      <p>
        Configurez une réponse, envoyez les formulations du chat et observez une décision
        explicable avant de la transmettre à votre workflow de jeu.
      </p>
    </div>
    <div class="stage-facts" aria-label="Propriétés du moteur">
      <span><Zap size={15} /> Synchrone</span>
      <span><ShieldCheck size={15} /> Hors réseau</span>
      <span><Braces size={15} /> Contrat JSON</span>
    </div>
  </section>

  <section class="workspace" aria-label="Console de validation">
    <aside class="panel configuration">
      <div class="panel-heading">
        <span class="step">01</span>
        <div><h2>Réponse attendue</h2><p>Contexte injecté pour cette manche</p></div>
      </div>

      <label for="canonical">Titre canonique</label>
      <input id="canonical" bind:value={canonical} maxlength="160" />

      <label for="aliases">Alias autorisés <span>un par ligne</span></label>
      <textarea id="aliases" bind:value={aliases} rows="5" maxlength="1200"></textarea>

      <div class="policy">
        <div><span>Seuil d’acceptation</span><strong>87%</strong></div>
        <div class="meter"><i style="width: 87%"></i></div>
        <p>Entre 72 et 87%, le moteur s’abstient et laisse le workflow arbitrer.</p>
      </div>
    </aside>

    <section class="panel test-bench">
      <div class="panel-heading">
        <span class="step">02</span>
        <div><h2>Message du chat</h2><p>Entrée non fiable à interpréter</p></div>
      </div>

      <div class="sample-row" aria-label="Exemples de réponses">
        {#each samples as sample}
          <button class:active={message === sample.value} onclick={() => (message = sample.value)}>
            {sample.label}
          </button>
        {/each}
      </div>

      <label for="message">Réponse utilisateur</label>
      <div class="message-field">
        <span>&gt;</span>
        <input
          id="message"
          bind:value={message}
          onkeydown={(event) => event.key === 'Enter' && runValidation()}
          maxlength="1000"
          autocomplete="off"
          spellcheck="false"
        />
      </div>

      <div class="identity-row">
        <div>
          <label for="participant">Participant</label>
          <input id="participant" bind:value={participant} maxlength="256" />
        </div>
        <div>
          <label for="sequence">Ordre source</label>
          <input id="sequence" type="number" bind:value={sequence} min="0" />
        </div>
      </div>

      <button class="run-button" onclick={runValidation} disabled={busy || !message.trim()}>
        <Play size={17} fill="currentColor" /> {busy ? 'Validation…' : 'Valider la réponse'}
        <kbd>↵</kbd>
      </button>

      {#if error}<p class="error" role="alert"><TriangleAlert size={16} /> {error}</p>{/if}
    </section>

    <aside class="panel result-panel" aria-live="polite">
      <div class="panel-heading">
        <span class="step">03</span>
        <div><h2>Décision</h2><p>Signal transmis au workflow</p></div>
      </div>

      {#if result}
        <div class:accepted={result.decision === 'accepted'} class:abstained={result.decision === 'abstained'} class:rejected={result.decision === 'rejected'} class="decision-card">
          <div class="decision-icon">
            {#if result.decision === 'accepted'}<Check size={24} />{:else if result.decision === 'abstained'}<TriangleAlert size={23} />{:else}<X size={24} />{/if}
          </div>
          <div><span>Résultat</span><strong>{decisionLabel(result.decision)}</strong></div>
          <b>{Math.round(result.score * 100)}%</b>
        </div>

        <dl class="telemetry">
          <div><dt>Latence UI</dt><dd>{result.latency.toFixed(2)} ms</dd></div>
          <div><dt>Participant</dt><dd>{result.participant_id}</dd></div>
          <div><dt>Ordre source</dt><dd>#{result.source_sequence}</dd></div>
          <div><dt>Preuve</dt><dd>{result.evidence[0]?.kind ?? 'aucune'}</dd></div>
        </dl>
      {:else}
        <div class="empty-result">
          <Database size={30} strokeWidth={1.5} />
          <strong>En attente d’un signal</strong>
          <p>La décision, le score et la preuve apparaîtront ici.</p>
        </div>
      {/if}

      <div class="workflow-note">
        <Radio size={16} />
        <p><strong>Le moteur ne choisit pas le gagnant.</strong> Il restitue l’ordre source pour que votre workflow récompense la première validation acceptée.</p>
      </div>
    </aside>
  </section>

  <section class="history-panel">
    <div class="history-heading">
      <div><h2>Journal de session</h2><p>Les huit dernières validations, en mémoire locale seulement.</p></div>
      <button onclick={resetSession} disabled={!history.length}><RotateCcw size={15} /> Réinitialiser</button>
    </div>
    {#if history.length}
      <div class="history-list">
        {#each history as item}
          <div class="history-item">
            <span class="decision-pip {item.decision}"></span>
            <code>{item.input}</code>
            <span>{item.participant_id}</span>
            <span class="history-decision">{decisionLabel(item.decision)}</span>
            <strong>{Math.round(item.score * 100)}%</strong>
            <time>{item.latency.toFixed(1)} ms</time>
          </div>
        {/each}
      </div>
    {:else}
      <p class="history-empty">Aucune validation dans cette session.</p>
    {/if}
  </section>
</main>
