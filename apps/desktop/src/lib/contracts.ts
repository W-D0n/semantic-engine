export type Decision = 'accepted' | 'rejected' | 'abstained';

export type AnswerTarget = {
  id: string;
  canonical: string;
  aliases: string[];
};

export type Round = {
  id: string;
  targets: AnswerTarget[];
  policy: {
    accept_threshold: number;
    review_threshold: number;
    ambiguity_margin: number;
  };
};

export type SessionSnapshot = {
  contract_version: 2;
  session_id: string;
  round_id: string;
  context_package_sha256: string | null;
  state: 'active' | 'ended';
  created_at_ms: number;
  ended_at_ms: number | null;
  latest_event_sequence: number;
};

export type ResumableSession = {
  snapshot: SessionSnapshot;
  round: Round;
  next_source_sequence: number;
};

export type SessionValidation = {
  round_id: string;
  message_id: string;
  participant_id: string;
  source_sequence: number;
  decision: Decision;
  target_id: string | null;
  score: number;
  evidence_kinds: Validation['evidence'][number]['kind'][];
  issue: Validation['issue'] | null;
};

export type SessionEvent = {
  contract_version: number;
  session_id: string;
  sequence: number;
  occurred_at_ms: number;
} & (
  | { type: 'session_started'; payload: { round_id: string; context_package_sha256: string | null } }
  | { type: 'validation_recorded'; payload: SessionValidation }
  | { type: 'resolution_recorded'; payload: OperatorResolution }
  | { type: 'session_ended' }
);

export type SessionEventsPage = {
  contract_version: number;
  session_id: string;
  earliest_available_sequence: number;
  latest_sequence: number;
  truncated: boolean;
  events: SessionEvent[];
};

export type Validation = {
  round_id: string;
  message_id: string;
  participant_id: string;
  source_sequence: number;
  decision: Decision;
  target_id: string | null;
  score: number;
  evidence: Array<{
    kind:
      | 'configured_expression'
      | 'normalized_expression'
      | 'memory_expression'
      | 'fuzzy_expression'
      | 'ambiguous_expression';
    matched_expression: string;
  }>;
  issue?: 'invalid_policy' | 'invalid_round' | 'invalid_submission';
};

export type OperatorResolution = {
  round_id: string;
  message_id: string;
  participant_id: string;
  source_sequence: number;
  original_decision: Decision;
  final_decision: Exclude<Decision, 'abstained'>;
  target_id: string | null;
  note: string;
};

export type MemoryEntry = {
  id: string;
  context_package_sha256: string;
  target_id: string;
  expression: string;
  normalized_expression: string;
  normalization_version: number;
  source_resolution_sha256: string;
  created_at_ms: number;
  last_used_at_ms: number;
  expires_at_ms: number;
  use_count: number;
  state: 'active' | 'revoked' | 'expired' | 'evicted';
  state_changed_at_ms: number | null;
};

export type HistoryItem = Validation & {
  input: string;
  latency: number;
  persisted?: boolean;
  resolution?: OperatorResolution;
};

export type AuditEntry = {
  schema_version: 2;
  validation: {
    sequence: number;
    recorded_at_ms: number;
    round_id: string;
    message_id: string;
    participant_id: string;
    source_sequence: number;
    context_package_sha256: string | null;
    decision: Decision;
    target_id: string | null;
    score: number;
    evidence_kinds: Validation['evidence'][number]['kind'][];
    issue: Validation['issue'] | null;
  };
  resolution: null | (Omit<OperatorResolution, 'round_id' | 'message_id' | 'participant_id' | 'source_sequence'> & {
    recorded_at_ms: number;
  });
};

export type ContextPackagePreview = {
  name: string;
  id: string;
  version: string;
  license: string;
  locales: string[];
  sources: Array<{
    title: string;
    path: string | null;
    version: string | null;
  }>;
  target_count: number;
  package_sha256: string;
  targets_sha256: string;
};

export type ChannelRootPreview = {
  sha256: string;
  version: number;
  expires: string;
  consistent_snapshot: boolean;
  root_threshold: number;
  root_key_ids: string[];
};

export type ContextChannelEnrollmentPreview = {
  root: ChannelRootPreview;
  already_trusted: boolean;
};

export type ContextTargetMetadata = {
  profile: string;
  package_id: string;
  package_name: string;
  package_version: string;
  format_version: string;
  kind: string;
  locales: string[];
  kinds: string[];
  target_count: number;
  spdx_license_expression: string;
};

export type ContextRevocation = {
  archive_sha256: string;
  package_id: string;
  package_version: string;
  effective_at: string;
  reason: 'compromised' | 'invalid-data' | 'legal' | 'withdrawn' | 'other';
  replacement?: string;
};

export type ContextChannelPackage = {
  target_path: string;
  archive_length: number;
  archive_sha256: string;
  metadata: ContextTargetMetadata;
  revocation: ContextRevocation | null;
};

export type VerifiedContextChannel = {
  channel: {
    $schema: string;
    formatVersion: number;
    id: string;
    name: string;
    homepage: string;
    packages: Array<{ path: string; metadata: ContextTargetMetadata }>;
  };
  root_sha256: string;
  root_version: number;
  timestamp_version: number;
  snapshot_version: number;
  targets_version: number;
  timestamp_expires: string;
  verified_at: string;
  revocation_sequence: number;
  revocations_updated_at: string;
  packages: ContextChannelPackage[];
};

export type ContextChannelVerificationOutcome = {
  verified: VerifiedContextChannel;
  quarantined_context: ContextPackagePreview | null;
};

export type ExportedContextPackage = {
  preview: ContextPackagePreview;
  descriptor_path: string;
};

export type TargetRecord = AnswerTarget & {
  is_draft: boolean;
  package_sha256: string;
};

export type LoopbackStatus = {
  running: boolean;
  address: string | null;
  token: string | null;
  protocol_version: number;
  allowed_origins: string[];
};

export type SourceDesiredState = 'paused' | 'active';
export type SourceRuntimeState =
  | 'paused'
  | 'authentication_required'
  | 'connecting'
  | 'connected'
  | 'backoff'
  | 'faulted';

export type SourceRuntimeSnapshot = {
  state: SourceRuntimeState | null;
  detail: string | null;
  fault: { code: string; retryable: boolean } | null;
  session_id: string | null;
  messages_received: number;
  accepted: number;
  last_event_at_ms: number | null;
};

export type SourceView = {
  contract_version: number;
  source_id: string;
  adapter: string;
  display_name: string;
  settings: Record<string, string>;
  credential_id: string | null;
  desired_state: SourceDesiredState;
  revision: number;
  created_at_ms: number;
  updated_at_ms: number;
  runtime: SourceRuntimeSnapshot;
  authenticated: boolean;
};

export type SourceDeletionReceipt = {
  source_id: string;
  adapter: string;
  provider_revocation: 'succeeded' | 'failed' | 'not_applicable';
  credential_purged: boolean;
  durable_source_purged: boolean;
  completed_at_ms: number;
};

export type DeviceAuthorizationPrompt = {
  user_code: string;
  verification_uri: string;
  expires_at_ms: number;
  poll_interval_seconds: number;
};

export type TwitchSourceTest = {
  login: string;
  user_id: string;
  expires_in_seconds: number;
};

export type TwitchAuthorizationStatus =
  | { status: 'pending'; prompt: DeviceAuthorizationPrompt }
  | { status: 'slow_down'; prompt: DeviceAuthorizationPrompt }
  | { status: 'authorized'; source: SourceView; identity: TwitchSourceTest };

export type BrowserAuthorizationPrompt = {
  authorization_uri: string;
  expires_at_ms: number;
};

export type YouTubeSourceTest = {
  channel_id: string;
  display_name: string;
  video_id: string;
};

export type YouTubeBroadcast = {
  video_id: string;
  title: string;
  scheduled_start_time: string | null;
  actual_start_time: string | null;
};

export type YouTubeAuthorizationStatus =
  | { status: 'pending'; prompt: BrowserAuthorizationPrompt }
  | { status: 'authorized'; source: SourceView; identity: YouTubeSourceTest };
