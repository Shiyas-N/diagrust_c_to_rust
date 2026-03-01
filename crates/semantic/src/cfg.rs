//! # Basic Control Flow Graph (CFG)
//!
//! Provides a control-flow-sensitive view of the function body, used
//! to guide ownership inference and lifetime computation.

use ast_input::*;

#[derive(Debug, Clone)]
pub enum CfgNode {
    Entry,
    Exit,
    Statement { index: usize, stmt_kind: StmtKind },
    Branch { condition_index: usize },
    LoopHeader { index: usize },
    LoopBack { header_index: usize },
    Merge { index: usize },
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    VarDecl,
    Assignment,
    IndexedAssignment,
    Return,
    Expression,
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub struct CfgEdge {
    pub from: usize,
    pub to: usize,
    pub edge_kind: EdgeKind,
}

#[derive(Debug, Clone)]
pub enum EdgeKind {
    Fallthrough,
    TrueEdge,
    FalseEdge,
    LoopBack,
    LoopExit,
    Return,
}

/// A simple basic-block Control Flow Graph for a single function.
#[derive(Debug, Clone, Default)]
pub struct ControlFlowGraph {
    pub nodes: Vec<CfgNode>,
    pub edges: Vec<CfgEdge>,
    pub entry: usize,
    pub exits: Vec<usize>,
    pub has_loops: bool,
    pub has_branches: bool,
}

impl ControlFlowGraph {
    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn edge_count(&self) -> usize { self.edges.len() }
}

pub struct CfgBuilder;

impl CfgBuilder {
    pub fn build(func: &CFunction) -> ControlFlowGraph {
        let mut cfg = ControlFlowGraph::default();

        // Node 0: Entry
        cfg.nodes.push(CfgNode::Entry);
        cfg.entry = 0;

        let mut idx = 1usize;
        Self::process_stmts(&func.body, &mut cfg, &mut idx);

        // Final exit node
        let exit_idx = cfg.nodes.len();
        cfg.nodes.push(CfgNode::Exit);
        cfg.exits.push(exit_idx);

        cfg
    }

    fn process_stmts(stmts: &[CStatement], cfg: &mut ControlFlowGraph, idx: &mut usize) {
        for stmt in stmts {
            let node_idx = cfg.nodes.len();
            match stmt {
                CStatement::VarDecl { .. } => {
                    cfg.nodes.push(CfgNode::Statement { index: *idx, stmt_kind: StmtKind::VarDecl });
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Fallthrough });
                }
                CStatement::Assignment { .. } => {
                    cfg.nodes.push(CfgNode::Statement { index: *idx, stmt_kind: StmtKind::Assignment });
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Fallthrough });
                }
                CStatement::If { then_body, else_body, .. } => {
                    cfg.has_branches = true;
                    cfg.nodes.push(CfgNode::Branch { condition_index: *idx });
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Fallthrough });
                    let branch_node = node_idx;
                    let mut sub = *idx + 1;
                    Self::process_stmts(then_body, cfg, &mut sub);
                    if let Some(eb) = else_body { Self::process_stmts(eb, cfg, &mut sub); }
                    let merge_node = cfg.nodes.len();
                    cfg.nodes.push(CfgNode::Merge { index: merge_node });
                    cfg.edges.push(CfgEdge { from: branch_node, to: merge_node, edge_kind: EdgeKind::FalseEdge });
                    *idx = sub;
                }
                CStatement::While { body, .. } => {
                    cfg.has_loops = true;
                    cfg.nodes.push(CfgNode::LoopHeader { index: *idx });
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Fallthrough });
                    let header = node_idx;
                    let mut sub = *idx + 1;
                    Self::process_stmts(body, cfg, &mut sub);
                    let back_node = cfg.nodes.len();
                    cfg.nodes.push(CfgNode::LoopBack { header_index: header });
                    cfg.edges.push(CfgEdge { from: back_node, to: header, edge_kind: EdgeKind::LoopBack });
                    *idx = sub;
                }
                CStatement::Return { .. } => {
                    cfg.nodes.push(CfgNode::Statement { index: *idx, stmt_kind: StmtKind::Return });
                    cfg.exits.push(cfg.nodes.len() - 1);
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Return });
                }
                _ => {
                    cfg.nodes.push(CfgNode::Statement { index: *idx, stmt_kind: StmtKind::Expression });
                    cfg.edges.push(CfgEdge { from: node_idx.saturating_sub(1), to: node_idx, edge_kind: EdgeKind::Fallthrough });
                }
            }
            *idx += 1;
        }
    }
}
