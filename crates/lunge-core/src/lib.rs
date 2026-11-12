//! UI-agnostic core for Lunge.
//!
//! This crate owns AWS config parsing, SSO session state, triggering
//! `aws sso login`, and console federation. It must never depend on a GUI
//! toolkit; `lunge-gui` is a thin front end over it.
//!
//! # Invariant: no credential custody
//!
//! Lunge never stores, encrypts, or persists credentials or tokens. The AWS
//! CLI owns all token persistence. This crate only *reads* the CLI's state
//! (`~/.aws/config`, `~/.aws/sso/cache/`) and shells out to the CLI to act.
//!
//! The single exception is console federation: short-lived role credentials
//! are held in memory only for the duration of the federation HTTP call and
//! are never written to disk, logged, or cached.
//!
//! Every code path that touches credential state must restate this invariant
//! in a comment at the point of contact.

pub mod cache;
pub mod config;
