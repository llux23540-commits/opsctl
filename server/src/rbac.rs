//! Authorization-rule engine (JumpServer-lite).
//!
//! `authorize(user, asset, action)` returns the `system_user` (account) to
//! connect with, or None if denied. Visibility = union of assets any of the
//! user's rules match (plus ancestor sites so the tree shows the path).

use std::collections::{HashMap, HashSet};

use crate::state::now_secs;
use crate::store::{RuleRow, Store};

fn actions(r: &RuleRow) -> Vec<String> {
    r.actions
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn valid_now(r: &RuleRow, now: i64) -> bool {
    now >= r.valid_from && r.valid_until.map_or(true, |u| now <= u)
}

/// asset_id is `node_id` itself or a descendant of it (via parent chain).
fn in_subtree(asset_id: &str, node_id: &str, parent: &HashMap<String, Option<String>>) -> bool {
    if asset_id == node_id {
        return true;
    }
    let mut cur = parent.get(asset_id).cloned().flatten();
    while let Some(p) = cur {
        if p == node_id {
            return true;
        }
        cur = parent.get(&p).cloned().flatten();
    }
    false
}

async fn rule_matches(
    store: &Store,
    r: &RuleRow,
    asset_id: &str,
    parent: &HashMap<String, Option<String>>,
) -> bool {
    match r.selector_kind.as_str() {
        "assets" => r.selector.split(',').any(|s| s.trim() == asset_id),
        "tag" => store
            .asset_tag_ids(asset_id)
            .await
            .unwrap_or_default()
            .iter()
            .any(|t| t == &r.selector),
        "subtree" => in_subtree(asset_id, &r.selector, parent),
        _ => false,
    }
}

fn parent_map(assets: &[crate::store::AssetRow]) -> HashMap<String, Option<String>> {
    assets
        .iter()
        .map(|a| (a.id.clone(), a.parent_id.clone()))
        .collect()
}

/// Outcome of an allowed authorization: which account to connect with, and
/// whether the matched rule requires approval before execution.
#[derive(Debug, Clone)]
pub struct Authz {
    pub account_id: String,
    pub needs_approval: bool,
    pub min_approvals: i64,
    /// CSV of designated approver ids (empty = any admin).
    pub approver_ids: String,
    /// Review channel of the matched rule: "console" | "tg".
    pub quick: String,
}

/// Returns the account + approval requirement if allowed, else None.
pub async fn authorize(
    store: &Store,
    user_id: &str,
    is_admin: bool,
    asset_id: &str,
    action: &str,
) -> Option<Authz> {
    let assets = store.list_assets().await.ok()?;
    let parent = parent_map(&assets);

    if is_admin {
        // Admin may run on any asset; use the first account bound to it. Admin
        // bypasses approval.
        let account_id = store
            .accounts_of_asset(asset_id)
            .await
            .ok()
            .and_then(|v| v.into_iter().next())?;
        return Some(Authz { account_id, needs_approval: false, min_approvals: 1, approver_ids: String::new(), quick: "console".into() });
    }

    let now = now_secs();
    for r in store.list_rules_for_user(user_id).await.ok()? {
        if valid_now(&r, now)
            && actions(&r).iter().any(|a| a == action)
            && rule_matches(store, &r, asset_id, &parent).await
        {
            let su = r.system_user_id.clone();
            let account_id = if su.is_empty() {
                store
                    .accounts_of_asset(asset_id)
                    .await
                    .ok()
                    .and_then(|v| v.into_iter().next())?
            } else {
                su
            };
            return Some(Authz {
                account_id,
                needs_approval: r.needs_approval != 0,
                min_approvals: r.min_approvals.max(1),
                approver_ids: r.approver_ids.clone(),
                quick: if r.quick.is_empty() { "console".into() } else { r.quick.clone() },
            });
        }
    }
    None
}

/// Set of asset ids the user may see (matched by any rule + ancestor sites).
pub async fn visible_asset_ids(store: &Store, user_id: &str, is_admin: bool) -> HashSet<String> {
    let assets = store.list_assets().await.unwrap_or_default();
    if is_admin {
        return assets.iter().map(|a| a.id.clone()).collect();
    }
    let parent = parent_map(&assets);
    let rules = store.list_rules_for_user(user_id).await.unwrap_or_default();
    let now = now_secs();
    let mut vis = HashSet::new();
    for a in &assets {
        for r in &rules {
            if valid_now(r, now) && rule_matches(store, r, &a.id, &parent).await {
                vis.insert(a.id.clone());
                // reveal ancestor path so the tree renders
                let mut cur = parent.get(&a.id).cloned().flatten();
                while let Some(p) = cur {
                    vis.insert(p.clone());
                    cur = parent.get(&p).cloned().flatten();
                }
                break;
            }
        }
    }
    vis
}
