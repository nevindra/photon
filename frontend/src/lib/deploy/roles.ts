// The role model behind every mutating button in the app. Three roles, ranked: a Viewer sees
// everything (today's single implicit role), an Operator may change what runs on a host through the
// approved paths (container lifecycle, stacks, rollouts from templates), an Admin may also change
// the paths themselves (templates, agents, arbitrary compose, exec, users).
//
// Pure and table-testable — `lib/core/auth.ts` owns the signed-in user's role and exposes `can()`
// over this table. The UI never hides a control a role lacks: it disables it and says which role
// would be needed (`permissionHint`), because a button that vanishes reads as a missing feature and
// a button that refuses reads as a policy.
export type Role = 'viewer' | 'operator' | 'admin'

export const ROLES: Role[] = ['viewer', 'operator', 'admin']

export const ROLE_RANK: Record<Role, number> = { viewer: 0, operator: 1, admin: 2 }

export interface RoleMeta {
  label: string
  /** One line for the role picker, in the words an admin would use to explain the choice. */
  summary: string
}

export const ROLE_META: Record<Role, RoleMeta> = {
  viewer: { label: 'Viewer', summary: 'Sees every signal and every host. Cannot change anything.' },
  operator: {
    label: 'Operator',
    summary: 'Runs container lifecycle, stacks and rollouts from approved templates.',
  },
  admin: {
    label: 'Admin',
    summary: 'Everything an operator can, plus templates, agents, arbitrary compose, exec and users.',
  },
}

// What a control is asking permission for. Coarser than the API's route list on purpose: the UI
// wants to answer "may this person press this", not "which route is behind it".
export type Permission =
  | 'lifecycle' // start / stop / restart / kill / remove / pull / recreate / prune
  | 'deploy' // stacks and rollouts from templates; reconcile drift
  | 'templates' // publish, import, edit templates and registries
  | 'agents' // enroll, rotate, remove agents; change poll interval
  | 'compose' // deploy arbitrary compose (not from a template)
  | 'exec' // shell into a container — the highest-risk feature in the product
  | 'users' // manage UI users and roles
  | 'audit' // read the audit log

const REQUIRED: Record<Permission, Role> = {
  lifecycle: 'operator',
  deploy: 'operator',
  templates: 'admin',
  agents: 'admin',
  compose: 'admin',
  exec: 'admin',
  users: 'admin',
  audit: 'viewer',
}

export function requiredRole(perm: Permission): Role {
  return REQUIRED[perm]
}

export function roleAllows(role: Role | null | undefined, perm: Permission): boolean {
  if (!role) return false
  return ROLE_RANK[role] >= ROLE_RANK[REQUIRED[perm]]
}

/** Tooltip copy for a disabled control: "Requires the Operator role". */
export function permissionHint(perm: Permission): string {
  return `Requires the ${ROLE_META[REQUIRED[perm]].label} role`
}

export function isRole(v: unknown): v is Role {
  return typeof v === 'string' && (ROLES as string[]).includes(v)
}
