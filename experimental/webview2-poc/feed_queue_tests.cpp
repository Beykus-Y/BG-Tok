#include "feed_queue.h"

#include <cassert>
#include <iostream>

using namespace bgtok;

static FeedItem MakeItem(const std::wstring& id, uint64_t pos, const std::wstring& type = L"video") {
    FeedItem it{};
    it.video_id = id;
    it.author = L"creator_" + id;
    it.title = L"desc_" + id;
    it.item_type = type;
    it.src_kind = L"xgwrapper";
    it.dom_pos = pos;
    it.observed_at_ms = 1000 + pos;
    return it;
}

int main() {
    // 1. Order, deduplication, non-video skip, and gap detection
    {
        FeedQueue q(2);
        assert(q.State() == FeedState::Empty);
        uint64_t gen = q.generation();

        assert(q.Push(MakeItem(L"100", 0), gen).accepted);
        assert(q.Push(MakeItem(L"100", 1), gen).duplicate);
        assert(q.Push(MakeItem(L"101", 1, L"ad"), gen).skipped_non_video);
        auto r_gap = q.Push(MakeItem(L"103", 3), gen);
        assert(r_gap.accepted && r_gap.gap_detected);

        assert(q.Length() == 2);
        assert(q.DuplicatesCount() == 1);
        assert(q.SkippedCount() == 1);
        assert(q.GapsCount() == 2);
        assert(q.LastVideoId() == L"103");
        assert(q.Current()->video_id == L"100");
    }

    // 2. Previous / Next, NeedsMoreItems, Exhausted, and appending next batch
    {
        FeedQueue q(1);
        uint64_t gen = q.generation();
        for (int i = 0; i < 4; ++i) {
            assert(q.Push(MakeItem(std::to_wstring(200 + i), static_cast<uint64_t>(i)), gen).accepted);
        }
        assert(q.State() == FeedState::Ready);
        assert(q.Remaining() == 3);
        assert(!q.Previous());
        assert(q.Next());
        assert(q.Current()->video_id == L"201");
        assert(q.Next());
        assert(q.Current()->video_id == L"202");
        assert(q.Remaining() == 1);
        assert(q.State() == FeedState::NeedsMoreItems);

        assert(q.Next());
        assert(!q.Next());
        q.MarkExhausted(true);
        assert(q.State() == FeedState::Exhausted);

        // Appending a new batch clears exhausted state
        assert(q.Push(MakeItem(L"204", 4), gen).accepted);
        assert(q.Push(MakeItem(L"205", 5), gen).accepted);
        assert(q.State() == FeedState::Ready);
        assert(q.Next());
        assert(q.Current()->video_id == L"204");
        assert(q.Previous());
        assert(q.Current()->video_id == L"203");
    }

    // 3. Account switch & stale generation cancellation
    {
        FeedQueue q(2);
        uint64_t gen1 = q.generation();
        assert(q.Push(MakeItem(L"300", 0), gen1).accepted);
        q.ResetForAccountSwitch();
        assert(q.Length() == 0);
        assert(q.State() == FeedState::Empty);
        assert(q.Current() == nullptr);

        auto stale = q.Push(MakeItem(L"301", 0), gen1);
        assert(!stale.accepted && stale.stale_generation);
    }

    std::cout << "All FeedQueue unit tests passed.\n";
    return 0;
}
