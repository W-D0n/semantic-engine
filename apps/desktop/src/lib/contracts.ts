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
  contract_version: 1;
  session_id: string;
  round_id: string;
  context_package_sha256: string | null;
  state: 'active' | 'ended';
  created_at_ms: number;
  ended_at_ms: number | null;
  latest_event_sequence: number;
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

export type HistoryItem = Validation & {
  input: string;
  latency: number;
  persisted?: boolean;
  resolution?: OperatorResolution;
};

export type AuditEntry = {
  schema_version: 1;
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

export type ExportedContextPackage = {
  preview: ContextPackagePreview;
  descriptor_path: string;
};

export type TargetRecord = AnswerTarget & {
  is_draft: boolean;
  package_sha256: string;
};
