export interface RateLimiter {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

export interface Env {
  RELAY_SECRET?: string;
  APNS_KEY_P8?: string;
  APNS_KEY_ID?: string;
  APNS_TEAM_ID?: string;
  APNS_BUNDLE_ID?: string;
  FCM_SERVICE_ACCOUNT_JSON?: string;
  RL_TOKEN?: RateLimiter;
  RL_IP?: RateLimiter;
  RL_REGISTER?: RateLimiter;
}

export type Priority = "high" | "normal";

export interface PushMessage {
  deviceToken: string;
  /** Opaque base64 ciphertext; never parsed here. */
  payload: string;
  priority: Priority;
  collapseId?: string;
}

/** `detail` is a short upstream status/reason for logs; it never holds payload or token. */
export type SendResult =
  | { kind: "sent" }
  | { kind: "unregistered"; detail: string }
  | { kind: "upstream_error"; detail: string };
