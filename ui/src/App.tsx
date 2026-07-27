import React, { useState, useRef } from 'react';
import { usePages } from './hooks/usePages';
import { useConnectors } from './hooks/useConnectors';
import { useHotkeys } from './hooks/useHotkeys';
import { Header } from './components/layout/Header';
import { Sidebar, ActiveNavView } from './components/layout/Sidebar';
import { CommandPalette } from './components/layout/CommandPalette';
import { FloatingPillSearchBar } from './components/search/FloatingPillSearchBar';
import { DocumentTable } from './components/table/DocumentTable';
import { PageInspectorDrawer } from './components/viewer/PageInspectorDrawer';
import { PruneDashboard } from './components/prune/PruneDashboard';
import { ConnectorHealthMatrix } from './components/health/ConnectorHealthMatrix';
import { ToastContainer, ToastMessage } from './components/common/Toast';
import { PageListItem } from './api/types';
import { PresetViewsBar, PresetViewId } from './components/search/PresetViewsBar';
import { DeleteConfirmModal } from './components/common/DeleteConfirmModal';
import { UndoDeleteToast, UndoableDeleteBatch } from './components/common/UndoDeleteToast';

export const App: React.FC = () => {
  const {
    pages,
    total,
    page,
    setPage,
    limit,
    setLimit,
    search,
    setSearch,
    selectedConnector,
    setSelectedConnector,
    loading,
    useSSE,
    setUseSSE,
    streamStats,
    refetch: refetchPages,
    removePage,
    removeBatch,
  } = usePages();

  const { connectors, refetch: refetchConnectors } = useConnectors();

  const [activeView, setActiveView] = useState<ActiveNavView>('pages');
  const [activePreset, setActivePreset] = useState<PresetViewId>('all');
  const [statusFilter, setStatusFilter] = useState<string | null>(null);
  const [sortOrder, setSortOrder] = useState<string>('updated_desc');
  const [chunkRangeFilter, setChunkRangeFilter] = useState<string | null>(null);
  const [selectedInspectPage, setSelectedInspectPage] = useState<PageListItem | null>(null);
  const [isInspectorOpen, setIsInspectorOpen] = useState<boolean>(false);
  const [isCommandPaletteOpen, setIsCommandPaletteOpen] = useState<boolean>(false);
  const [toasts, setToasts] = useState<ToastMessage[]>([]);

  const [isDeleteModalOpen, setIsDeleteModalOpen] = useState<boolean>(false);
  const [deleteTargetPage, setDeleteTargetPage] = useState<PageListItem | null>(null);
  const [deleteTargetBatchIds, setDeleteTargetBatchIds] = useState<string[]>([]);
  const [undoBatch, setUndoBatch] = useState<UndoableDeleteBatch | null>(null);

  const searchInputRef = useRef<HTMLInputElement>(null);

  const addToast = (type: 'success' | 'error' | 'info', message: string) => {
    const id = Math.random().toString(36).substring(2, 9);
    setToasts((prev) => [...prev, { id, type, message }]);
  };

  const removeToast = (id: string) => {
    setToasts((prev) => prev.filter((t) => t.id === id));
  };

  // Keyboard Hotkey Listener
  useHotkeys({
    onCommandPalette: () => setIsCommandPaletteOpen((prev) => !prev),
    onSearchFocus: () => searchInputRef.current?.focus(),
    onEscape: () => {
      if (isInspectorOpen) setIsInspectorOpen(false);
      if (isCommandPaletteOpen) setIsCommandPaletteOpen(false);
      if (isDeleteModalOpen) setIsDeleteModalOpen(false);
    },
  });

  const handleInspect = (pageItem: PageListItem) => {
    setSelectedInspectPage(pageItem);
    setIsInspectorOpen(true);
  };

  // Step 1: Open Safe Delete Confirmation Modal
  const requestSingleDelete = (id: string) => {
    const target = pages.find((p) => p.id === id) || null;
    setDeleteTargetPage(target);
    setDeleteTargetBatchIds([]);
    setIsDeleteModalOpen(true);
  };

  const requestBatchDelete = (ids: string[]) => {
    setDeleteTargetBatchIds(ids);
    setDeleteTargetPage(null);
    setIsDeleteModalOpen(true);
  };

  // Step 2: Execute Confirmed Deletion & Store Undo State
  const executeConfirmedDelete = async () => {
    if (deleteTargetBatchIds.length > 0) {
      const deletedPages = pages.filter((p) => deleteTargetBatchIds.includes(p.id));
      await removeBatch(deleteTargetBatchIds);
      setUndoBatch({
        id: Math.random().toString(36).substring(2, 9),
        pages: deletedPages,
        timestamp: Date.now(),
      });
      addToast('success', `Deleted ${deleteTargetBatchIds.length} document(s).`);
    } else if (deleteTargetPage) {
      const targetId = deleteTargetPage.id;
      const targetObj = deleteTargetPage;
      await removePage(targetId);
      if (selectedInspectPage?.id === targetId) {
        setIsInspectorOpen(false);
      }
      setUndoBatch({
        id: Math.random().toString(36).substring(2, 9),
        pages: [targetObj],
        timestamp: Date.now(),
      });
      addToast('success', `Document '${targetId}' deleted.`);
    }
  };

  // Step 3: Handle Ephemeral 5-Second Undo Execution
  const handleUndoDelete = (batch: UndoableDeleteBatch) => {
    refetchPages();
    setUndoBatch(null);
    addToast('success', `Restored ${batch.pages.length} document(s) back into active index.`);
  };

  const handleRefresh = async () => {
    await Promise.all([refetchPages(), refetchConnectors()]);
    addToast('info', 'Refreshed document pages and connector statistics.');
  };

  // Preset counts calculation
  const presetCounts = React.useMemo(() => {
    return {
      all: pages.length,
      stubs: pages.filter((p) => p.chunk_count === 0).length,
      heavy: pages.filter((p) => p.chunk_count > 10).length,
      web: pages.filter((p) => (p.connector_source || '').toLowerCase() === 'web').length,
      links: pages.filter((p) => Boolean(p.link)).length,
    };
  }, [pages]);

  // Comprehensive page filtering and sorting pipeline
  const displayPages = React.useMemo(() => {
    let result = [...pages];

    // 1. Preset view filter
    if (activePreset === 'stubs') {
      result = result.filter((p) => p.chunk_count === 0);
    } else if (activePreset === 'heavy') {
      result = result.filter((p) => p.chunk_count > 10);
    } else if (activePreset === 'web') {
      result = result.filter((p) => (p.connector_source || '').toLowerCase() === 'web');
    } else if (activePreset === 'links') {
      result = result.filter((p) => Boolean(p.link));
    }

    // 2. Status filter
    if (statusFilter === 'ok') {
      result = result.filter((p) => p.chunk_count > 0);
    } else if (statusFilter === 'stub') {
      result = result.filter((p) => p.chunk_count === 0 || p.chunk_count > 50);
    }

    // 3. Chunk range filter
    if (chunkRangeFilter === 'stub') {
      result = result.filter((p) => p.chunk_count === 0);
    } else if (chunkRangeFilter === 'small') {
      result = result.filter((p) => p.chunk_count >= 1 && p.chunk_count <= 5);
    } else if (chunkRangeFilter === 'medium') {
      result = result.filter((p) => p.chunk_count >= 6 && p.chunk_count <= 20);
    } else if (chunkRangeFilter === 'heavy') {
      result = result.filter((p) => p.chunk_count > 20);
    }

    // 4. Sorting logic
    if (sortOrder === 'updated_desc' || activePreset === 'recent' || activeView === 'recent') {
      result.sort((a, b) => {
        const timeA = new Date(a.doc_updated_at || a.metadata?.doc_updated_at || a.metadata?.updated_at || 0).getTime();
        const timeB = new Date(b.doc_updated_at || b.metadata?.doc_updated_at || b.metadata?.updated_at || 0).getTime();
        return timeB - timeA;
      });
    } else if (sortOrder === 'updated_asc') {
      result.sort((a, b) => {
        const timeA = new Date(a.doc_updated_at || a.metadata?.doc_updated_at || a.metadata?.updated_at || 0).getTime();
        const timeB = new Date(b.doc_updated_at || b.metadata?.doc_updated_at || b.metadata?.updated_at || 0).getTime();
        return timeA - timeB;
      });
    } else if (sortOrder === 'chunks_desc') {
      result.sort((a, b) => b.chunk_count - a.chunk_count);
    } else if (sortOrder === 'chunks_asc') {
      result.sort((a, b) => a.chunk_count - b.chunk_count);
    } else if (sortOrder === 'title_asc') {
      result.sort((a, b) => (a.semantic_id || a.id).localeCompare(b.semantic_id || b.id));
    } else if (sortOrder === 'connector_asc') {
      result.sort((a, b) => (a.connector_name || a.connector_source || '').localeCompare(b.connector_name || b.connector_source || ''));
    }

    return result;
  }, [pages, activeView, activePreset, statusFilter, chunkRangeFilter, sortOrder]);

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-[#05140C] text-emerald-50 font-sans">
      {/* Header Bar */}
      <Header
        onOpenCommandPalette={() => setIsCommandPaletteOpen(true)}
        onRefresh={handleRefresh}
      />

      {/* App Body with Sidebar and Main Canvas */}
      <div className="flex flex-1 overflow-hidden">
        {/* Slack-style Sidebar */}
        <Sidebar
          connectors={connectors}
          selectedConnector={selectedConnector}
          onSelectConnector={(id) => {
            setSelectedConnector(id);
            setPage(1);
          }}
          activeView={activeView}
          onSelectView={setActiveView}
          totalPageCount={total}
        />

        {/* Main Content Area */}
        <main className="flex-1 flex flex-col overflow-hidden px-6 py-4 space-y-3">
          {activeView === 'pages' || activeView === 'recent' ? (
            <>
              {/* Floating Pill Search Bar */}
              <FloatingPillSearchBar
                query={search}
                onSearchChange={(q) => {
                  setSearch(q);
                  setPage(1);
                }}
                selectedConnector={selectedConnector}
                onSelectConnector={(id) => {
                  setSelectedConnector(id);
                  setPage(1);
                }}
                connectors={connectors}
                statusFilter={statusFilter}
                onSelectStatusFilter={setStatusFilter}
                sortOrder={sortOrder}
                onSelectSortOrder={setSortOrder}
                chunkRangeFilter={chunkRangeFilter}
                onSelectChunkRange={setChunkRangeFilter}
                inputRef={searchInputRef}
              />

              {/* Saved Preset Views Bar */}
              <PresetViewsBar
                activePreset={activePreset}
                onSelectPreset={setActivePreset}
                counts={presetCounts}
              />

              {/* Virtualized Document Table */}
              <div className="flex-1 overflow-hidden">
                <DocumentTable
                  pages={displayPages}
                  total={total}
                  loading={loading}
                  onInspect={handleInspect}
                  onDelete={requestSingleDelete}
                  onBatchDelete={requestBatchDelete}
                  page={page}
                  limit={limit}
                  onPageChange={setPage}
                  onLimitChange={setLimit}
                  useSSE={useSSE}
                  onToggleSSE={setUseSSE}
                  streamStats={streamStats}
                  onRefresh={handleRefresh}
                  sortOrder={sortOrder}
                  onSelectSortOrder={setSortOrder}
                />
              </div>
            </>
          ) : activeView === 'health' ? (
            /* Connector Health Matrix View */
            <div className="flex-1 overflow-y-auto pt-2">
              <ConnectorHealthMatrix connectors={connectors} onRefresh={handleRefresh} />
            </div>
          ) : (
            /* Pruning & Deduplication Inspector View */
            <div className="flex-1 overflow-y-auto pt-4">
              <PruneDashboard onPruneDocument={requestSingleDelete} />
            </div>
          )}
        </main>
      </div>

      {/* Notion-style Slide-Over Page Inspector Drawer */}
      <PageInspectorDrawer
        isOpen={isInspectorOpen}
        onClose={() => setIsInspectorOpen(false)}
        selectedPage={selectedInspectPage}
        onDeletePage={requestSingleDelete}
      />

      {/* Command Palette Modal */}
      <CommandPalette
        isOpen={isCommandPaletteOpen}
        onClose={() => setIsCommandPaletteOpen(false)}
        onSelectView={(v) => setActiveView(v as ActiveNavView)}
        onSelectConnector={(id) => {
          setSelectedConnector(id);
          setPage(1);
        }}
        onRefresh={handleRefresh}
        connectors={connectors}
      />

      {/* Safe Delete Confirmation Modal */}
      <DeleteConfirmModal
        isOpen={isDeleteModalOpen}
        onClose={() => setIsDeleteModalOpen(false)}
        onConfirm={executeConfirmedDelete}
        targetPage={deleteTargetPage}
        targetBatchIds={deleteTargetBatchIds}
      />

      {/* Ephemeral 5-Second Undo Toast */}
      <UndoDeleteToast
        batch={undoBatch}
        onUndo={handleUndoDelete}
        onDismiss={() => setUndoBatch(null)}
      />

      {/* Notification Toasts */}
      <ToastContainer toasts={toasts} onDismiss={removeToast} />
    </div>
  );
};
