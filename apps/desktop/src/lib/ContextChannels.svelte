<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { open } from '@tauri-apps/plugin-dialog';
  import {
    BadgeCheck,
    Ban,
    Check,
    ChevronRight,
    Fingerprint,
    FolderSearch,
    KeyRound,
    PackageSearch,
    RefreshCw,
    ShieldAlert,
    TriangleAlert,
  } from '@lucide/svelte';
  import type {
    ContextChannelEnrollmentPreview,
    ContextChannelPackage,
    ContextChannelVerificationOutcome,
    ContextPackagePreview,
    VerifiedContextChannel,
  } from './contracts';

  let {
    inTauri,
    onContextQuarantined,
  }: {
    inTauri: boolean;
    onContextQuarantined?: (context: ContextPackagePreview) => void | Promise<void>;
  } = $props();

  let directory = $state('');
  let preview = $state<ContextChannelEnrollmentPreview | null>(null);
  let verified = $state<VerifiedContextChannel | null>(null);
  let quarantinedContext = $state<ContextPackagePreview | null>(null);
  let operation = $state<'idle' | 'preview' | 'verify'>('idle');
  let error = $state('');

  const displayedPackages = $derived(verified?.packages.slice(0, 100) ?? []);

  function shortHash(value: string, width = 12) {
    return value.length > width * 2 + 1
      ? `${value.slice(0, width)}…${value.slice(-width)}`
      : value;
  }

  function formatDate(value: string) {
    const date = new Date(value);
    return Number.isNaN(date.valueOf())
      ? value
      : new Intl.DateTimeFormat('fr-FR', {
          dateStyle: 'medium',
          timeStyle: 'short',
        }).format(date);
  }

  function bytes(value: number) {
    if (value < 1024 * 1024) return `${Math.ceil(value / 1024)} Kio`;
    return `${(value / (1024 * 1024)).toFixed(1)} Mio`;
  }

  function explain(cause: unknown) {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function chooseChannel() {
    if (!inTauri || operation !== 'idle') return;
    error = '';
    const selected = await open({
      directory: true,
      multiple: false,
      title: 'Choisir un canal de contextes signé',
    });
    if (typeof selected !== 'string') return;
    directory = selected;
    preview = null;
    verified = null;
    quarantinedContext = null;
    operation = 'preview';
    try {
      preview = await invoke<ContextChannelEnrollmentPreview>(
        'preview_context_channel_root_ipc',
        { channelDirectory: selected },
      );
    } catch (cause) {
      error = `Racine impossible à inspecter : ${explain(cause)}`;
    } finally {
      operation = 'idle';
    }
  }

  async function verifyChannel() {
    if (!preview || !directory || operation !== 'idle') return;
    operation = 'verify';
    error = '';
    verified = null;
    try {
      const outcome = await invoke<ContextChannelVerificationOutcome>('verify_context_channel_ipc', {
        channelDirectory: directory,
        expectedRootSha256: preview.root.sha256,
      });
      verified = outcome.verified;
      quarantinedContext = outcome.quarantined_context;
      if (outcome.quarantined_context) {
        await onContextQuarantined?.(outcome.quarantined_context);
      }
    } catch (cause) {
      error = `Canal refusé : ${explain(cause)} Vérifiez sa date, ses signatures et son contenu, puis réessayez.`;
    } finally {
      operation = 'idle';
    }
  }

  function packageState(item: ContextChannelPackage) {
    return item.revocation ? `Révoqué · ${item.revocation.reason}` : 'Disponible';
  }
</script>

<section class="channel-console" aria-labelledby="channel-heading">
  <header class="channel-heading">
    <div class="channel-title">
      <span class="channel-icon"><PackageSearch size={21} /></span>
      <div>
        <p class="channel-eyebrow">Distribution vérifiable</p>
        <h2 id="channel-heading">Canaux de contextes</h2>
        <p>Inspectez une source signée avant de lui accorder votre confiance. Aucun paquet n’est activé pendant cette étape.</p>
      </div>
    </div>
    <button class="choose-channel" onclick={chooseChannel} disabled={!inTauri || operation !== 'idle'}>
      <FolderSearch size={16} />
      {operation === 'preview' ? 'Inspection…' : preview ? 'Changer de canal' : 'Choisir un canal local'}
    </button>
  </header>

  {#if !inTauri}
    <div class="channel-notice muted-notice">
      <span class="notice-icon"><ShieldAlert size={17} /></span>
      <p><strong>Aperçu uniquement.</strong> La vérification cryptographique est disponible dans l’application portable.</p>
    </div>
  {/if}

  {#if preview}
    <div class="trust-review">
      <div class="trust-copy">
        <span class="trust-label"><Fingerprint size={15} /> Empreinte de confiance</span>
        <code title={preview.root.sha256}>{preview.root.sha256}</code>
        <p>
          Une signature valide prouve la cohérence de cette racine, pas l’identité de son éditeur.
          Comparez cette empreinte avec une valeur publiée par l’éditeur sur un canal indépendant.
        </p>
      </div>
      <dl class="root-facts">
        <div><dt>Racine</dt><dd>v{preview.root.version}</dd></div>
        <div><dt>Seuil</dt><dd>{preview.root.root_threshold}/{preview.root.root_key_ids.length} clé(s)</dd></div>
        <div><dt>Expiration</dt><dd>{formatDate(preview.root.expires)}</dd></div>
        <div><dt>Statut local</dt><dd>{preview.already_trusted ? 'Déjà approuvée' : 'Nouvelle confiance'}</dd></div>
      </dl>
      <div class="key-summary">
        <KeyRound size={15} />
        <span>{preview.root.root_key_ids.length} identité(s) de clé</span>
        <code title={preview.root.root_key_ids.join('\n')}>{shortHash(preview.root.root_key_ids[0] ?? 'absente', 9)}</code>
      </div>
      <button class="verify-channel" onclick={verifyChannel} disabled={operation !== 'idle'}>
        {#if operation === 'verify'}<span class="spin"><RefreshCw size={16} /></span>{:else}<Check size={16} />{/if}
        {operation === 'verify'
          ? 'Vérification complète…'
          : preview.already_trusted
            ? 'Vérifier le canal'
            : 'Approuver cette racine et vérifier'}
      </button>
    </div>
  {/if}

  {#if verified}
    <div class="verified-channel" aria-live="polite">
      <div class="verified-summary">
        <div class="verified-name">
          <span class="verified-icon"><BadgeCheck size={22} /></span>
          <div>
            <span>Signatures et fraîcheur validées</span>
            <h3>{verified.channel.name}</h3>
            <code>{verified.channel.id}</code>
          </div>
        </div>
        <dl>
          <div><dt>Paquets</dt><dd>{verified.packages.length}</dd></div>
          <div><dt>Index</dt><dd>v{verified.targets_version}</dd></div>
          <div><dt>Révocations</dt><dd>séq. {verified.revocation_sequence}</dd></div>
          <div><dt>Valide jusqu’au</dt><dd>{formatDate(verified.timestamp_expires)}</dd></div>
        </dl>
      </div>

      <div class="package-list-heading">
        <div>
          <h3>Paquets annoncés</h3>
          <p>Les archives restent inactives tant que vous ne les avez pas importées dans le contexte de reconnaissance.</p>
        </div>
        {#if verified.packages.length > displayedPackages.length}
          <span>{displayedPackages.length} affichés sur {verified.packages.length}</span>
        {/if}
      </div>

      <div class="package-list" aria-label="Paquets vérifiés du canal">
        {#each displayedPackages as item (item.target_path)}
          <article class:revoked={Boolean(item.revocation)}>
            <div class="package-main">
              <span class="package-state-icon">{#if item.revocation}<Ban size={17} />{:else}<BadgeCheck size={17} />{/if}</span>
              <div>
                <span>{packageState(item)}</span>
                <h4>{item.metadata.package_name} <small>v{item.metadata.package_version}</small></h4>
                <code title={item.target_path}>{item.target_path}</code>
              </div>
            </div>
            <div class="package-metadata">
              <span>{item.metadata.target_count.toLocaleString('fr-FR')} réponses</span>
              <span>{item.metadata.kinds.join(' · ')}</span>
              <span>{item.metadata.locales.join(', ')}</span>
              <span>{item.metadata.spdx_license_expression}</span>
              <span>{bytes(item.archive_length)}</span>
            </div>
            <span class="package-chevron"><ChevronRight size={16} aria-hidden="true" /></span>
          </article>
        {/each}
      </div>
    </div>
  {/if}

  {#if quarantinedContext}
    <div class="channel-notice quarantine-notice" role="alert" aria-live="assertive">
      <span class="notice-icon"><Ban size={17} /></span>
      <p>
        <strong>Contexte mis en quarantaine.</strong>
        {quarantinedContext.name} v{quarantinedContext.version} a été désactivé après vérification de sa révocation signée.
      </p>
    </div>
  {/if}

  {#if error}
    <div class="channel-notice error-notice" role="alert">
      <span class="notice-icon"><TriangleAlert size={17} /></span>
      <p><strong>Vérification interrompue.</strong> {error}</p>
    </div>
  {/if}
</section>

<style>
  .channel-console { border-bottom: 1px solid var(--line); background: #171a15; }
  .channel-heading { display: flex; align-items: center; justify-content: space-between; gap: 24px; padding: 24px clamp(24px, 4vw, 58px); }
  .channel-title { display: flex; align-items: flex-start; gap: 14px; min-width: 0; }
  .channel-icon { width: 40px; height: 40px; flex: 0 0 auto; display: grid; place-items: center; color: var(--lime); border: 1px solid #3a422e; background: #1b2115; }
  .channel-eyebrow { margin: 0 0 6px; color: var(--lime); font: 700 10px/1.2 "Cascadia Code", Consolas, monospace; letter-spacing: .1em; text-transform: uppercase; }
  h2, h3, h4, p { margin-top: 0; }
  h2 { margin-bottom: 0; font-size: 15px; letter-spacing: -.01em; }
  .channel-title > div > p:last-child { max-width: 730px; margin: 5px 0 0; color: var(--muted); font-size: 11px; line-height: 1.5; }
  button { min-height: 42px; display: inline-flex; align-items: center; justify-content: center; gap: 8px; border: 1px solid #596746; border-radius: 4px; padding: 10px 14px; color: var(--lime); background: #1d2416; cursor: pointer; font-size: 11px; font-weight: 750; }
  button:hover { border-color: var(--lime); background: #242d1a; }
  button:disabled { color: #737b6b; border-color: #33382e; background: #1a1d18; cursor: not-allowed; }
  .choose-channel { flex: 0 0 auto; }
  .trust-review { display: grid; grid-template-columns: minmax(280px, 1.4fr) minmax(330px, 1fr) auto; align-items: center; gap: 18px 28px; padding: 22px clamp(24px, 4vw, 58px); border-top: 1px solid var(--line); background: #141712; }
  .trust-copy { min-width: 0; }
  .trust-label { display: flex; align-items: center; gap: 7px; color: #d8decf; font-size: 11px; font-weight: 750; }
  .trust-copy code { display: block; margin-top: 8px; color: var(--lime); font: 600 10px/1.45 "Cascadia Code", Consolas, monospace; overflow-wrap: anywhere; }
  .trust-copy p { max-width: 680px; margin: 8px 0 0; color: #8c9386; font-size: 10px; line-height: 1.55; }
  .root-facts { display: grid; grid-template-columns: repeat(2, minmax(110px, 1fr)); gap: 13px 20px; margin: 0; }
  dt { color: #747b70; font-size: 9px; text-transform: uppercase; letter-spacing: .08em; }
  dd { margin: 4px 0 0; color: #cbd0c5; font: 600 10px/1.35 "Cascadia Code", Consolas, monospace; overflow-wrap: anywhere; }
  .key-summary { grid-column: 1 / 3; display: flex; align-items: center; gap: 8px; color: #8f9789; font-size: 10px; }
  .key-summary code { color: #b5bcaf; font: 600 9px/1.3 "Cascadia Code", Consolas, monospace; }
  .verify-channel { grid-column: 3; grid-row: 1 / 3; max-width: 235px; color: #161910; border-color: var(--lime); background: var(--lime); }
  .verify-channel:hover { color: #161910; background: #d5fa72; }
  .verified-channel { border-top: 1px solid var(--line); }
  .verified-summary { display: grid; grid-template-columns: minmax(240px, .8fr) minmax(500px, 1.4fr); align-items: center; gap: 28px; padding: 22px clamp(24px, 4vw, 58px); background: #1a1f16; }
  .verified-name { display: flex; align-items: flex-start; gap: 11px; min-width: 0; }
  .verified-icon { flex: 0 0 auto; color: var(--lime); }
  .verified-name span { color: var(--lime); font-size: 10px; font-weight: 750; text-transform: uppercase; letter-spacing: .07em; }
  .verified-name h3 { margin: 5px 0 2px; font-size: 15px; overflow-wrap: anywhere; }
  .verified-name code { color: #858d80; font: 500 9px/1.35 "Cascadia Code", Consolas, monospace; overflow-wrap: anywhere; }
  .verified-summary dl { display: grid; grid-template-columns: repeat(4, minmax(90px, 1fr)); gap: 16px; margin: 0; }
  .package-list-heading { display: flex; align-items: flex-end; justify-content: space-between; gap: 20px; padding: 20px clamp(24px, 4vw, 58px) 12px; }
  .package-list-heading h3 { margin-bottom: 4px; font-size: 13px; }
  .package-list-heading p, .package-list-heading > span { margin: 0; color: #7f877a; font-size: 10px; line-height: 1.45; }
  .package-list { padding: 0 clamp(24px, 4vw, 58px) 24px; }
  .package-list article { display: grid; grid-template-columns: minmax(250px, 1.3fr) minmax(420px, 1fr) auto; align-items: center; gap: 20px; min-height: 72px; padding: 13px 15px; border: 1px solid #30352d; border-bottom: 0; background: #151814; }
  .package-list article:first-child { border-radius: 4px 4px 0 0; }
  .package-list article:last-child { border-bottom: 1px solid #30352d; border-radius: 0 0 4px 4px; }
  .package-list article:only-child { border-radius: 4px; }
  .package-list article.revoked { border-color: #4a302d; background: #1e1715; }
  .package-main { display: flex; align-items: flex-start; gap: 10px; min-width: 0; }
  .package-state-icon { flex: 0 0 auto; color: var(--lime); }
  .revoked .package-state-icon, .revoked .package-main span { color: var(--red); }
  .package-main span { display: block; color: var(--lime); font-size: 9px; font-weight: 750; text-transform: uppercase; letter-spacing: .07em; }
  .package-main h4 { margin: 4px 0 2px; color: #d7dbd1; font-size: 11px; overflow-wrap: anywhere; }
  .package-main small { color: #8c9386; font: 600 9px/1 "Cascadia Code", Consolas, monospace; }
  .package-main code { display: block; max-width: 650px; color: #71796e; font: 500 8px/1.4 "Cascadia Code", Consolas, monospace; overflow-wrap: anywhere; }
  .package-metadata { display: flex; flex-wrap: wrap; gap: 5px 12px; color: #a0a79b; font: 500 9px/1.4 "Cascadia Code", Consolas, monospace; }
  .package-chevron { color: #596154; }
  .channel-notice { display: flex; align-items: flex-start; gap: 9px; padding: 15px clamp(24px, 4vw, 58px); border-top: 1px solid var(--line); font-size: 11px; line-height: 1.5; }
  .notice-icon { flex: 0 0 auto; margin-top: 1px; }
  .channel-notice p { margin: 0; }
  .muted-notice { color: #9ba294; background: #141712; }
  .error-notice { color: #e9948c; border-color: #392825; background: #1d1614; }
  .error-notice strong { color: #ffada5; }
  .quarantine-notice { color: #f0b0aa; border-color: #54332f; background: #211715; }
  .quarantine-notice strong { color: #ffada5; }
  .spin { animation: spin 900ms linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
  @media (max-width: 980px) {
    .trust-review { grid-template-columns: 1fr; }
    .root-facts { grid-template-columns: repeat(4, minmax(90px, 1fr)); }
    .key-summary, .verify-channel { grid-column: 1; grid-row: auto; }
    .verify-channel { max-width: none; justify-self: start; }
    .verified-summary { grid-template-columns: 1fr; }
    .package-list article { grid-template-columns: 1fr auto; }
    .package-metadata { grid-column: 1; }
    .package-chevron { grid-column: 2; grid-row: 1 / 3; }
  }
  @media (max-width: 640px) {
    .channel-heading { align-items: stretch; flex-direction: column; }
    .choose-channel { width: 100%; }
    .root-facts, .verified-summary dl { grid-template-columns: repeat(2, minmax(100px, 1fr)); }
    .package-list-heading { align-items: flex-start; flex-direction: column; }
    .package-list article { grid-template-columns: minmax(0, 1fr); }
    .package-chevron { display: none; }
  }
</style>
