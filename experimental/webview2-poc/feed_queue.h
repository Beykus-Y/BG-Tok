#pragma once

#include <cstdint>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace bgtok {

enum class FeedState {
    Empty,
    Ready,
    NeedsMoreItems,
    Exhausted,
};

inline const wchar_t* FeedStateToString(FeedState s) {
    switch (s) {
        case FeedState::Empty: return L"EMPTY";
        case FeedState::Ready: return L"READY";
        case FeedState::NeedsMoreItems: return L"NEEDS_MORE_ITEMS";
        case FeedState::Exhausted: return L"EXHAUSTED";
    }
    return L"UNKNOWN";
}

struct FeedItem {
    std::wstring video_id;
    std::wstring author;
    std::wstring title;
    std::wstring item_type;  // "video", "ad", "live"
    std::wstring src_kind;   // "href", "xgwrapper", "href-fallback"
    uint64_t dom_pos = 0;
    uint64_t observed_at_ms = 0;
};

struct PushResult {
    bool accepted = false;
    bool duplicate = false;
    bool stale_generation = false;
    bool skipped_non_video = false;
    bool gap_detected = false;
};

class FeedQueue {
public:
    explicit FeedQueue(size_t low_water_mark = 3) : low_water_mark_(low_water_mark) {}

    uint64_t generation() const { return generation_; }

    void ResetForAccountSwitch() {
        generation_++;
        items_.clear();
        seen_ids_.clear();
        current_index_.reset();
        last_dom_pos_.reset();
        last_video_id_.clear();
        duplicates_count_ = 0;
        skipped_count_ = 0;
        gaps_count_ = 0;
        exhausted_ = false;
    }

    void MarkExhausted(bool exhausted) { exhausted_ = exhausted; }

    PushResult Push(const FeedItem& item, uint64_t expected_generation) {
        PushResult res{};
        if (expected_generation != generation_) {
            res.stale_generation = true;
            return res;
        }
        if (item.video_id.empty()) {
            return res;
        }
        if (item.item_type == L"ad" || item.item_type == L"live") {
            skipped_count_++;
            res.skipped_non_video = true;
            return res;
        }
        if (!seen_ids_.insert(item.video_id).second) {
            duplicates_count_++;
            res.duplicate = true;
            return res;
        }

        if (last_dom_pos_.has_value() && item.dom_pos > (*last_dom_pos_ + 1)) {
            gaps_count_ += (item.dom_pos - *last_dom_pos_ - 1);
            res.gap_detected = true;
        }
        last_dom_pos_ = item.dom_pos;
        last_video_id_ = item.video_id;

        items_.push_back(item);
        if (!current_index_.has_value()) {
            current_index_ = 0;
        }
        exhausted_ = false;
        res.accepted = true;
        return res;
    }

    bool Next() {
        if (!current_index_.has_value()) return false;
        if (*current_index_ + 1 < items_.size()) {
            (*current_index_)++;
            return true;
        }
        return false;
    }

    bool Previous() {
        if (!current_index_.has_value()) return false;
        if (*current_index_ > 0) {
            (*current_index_)--;
            return true;
        }
        return false;
    }

    const FeedItem* Current() const {
        if (!current_index_.has_value() || *current_index_ >= items_.size()) return nullptr;
        return &items_[*current_index_];
    }

    size_t Length() const { return items_.size(); }

    std::optional<size_t> CurrentIndex() const { return current_index_; }

    size_t Remaining() const {
        if (!current_index_.has_value() || items_.empty()) return 0;
        return (items_.size() - 1) - *current_index_;
    }

    FeedState State() const {
        if (items_.empty()) {
            return exhausted_ ? FeedState::Exhausted : FeedState::Empty;
        }
        if (exhausted_ && Remaining() == 0) {
            return FeedState::Exhausted;
        }
        if (Remaining() <= low_water_mark_) {
            return FeedState::NeedsMoreItems;
        }
        return FeedState::Ready;
    }

    uint64_t DuplicatesCount() const { return duplicates_count_; }
    uint64_t SkippedCount() const { return skipped_count_; }
    uint64_t GapsCount() const { return gaps_count_; }
    const std::wstring& LastVideoId() const { return last_video_id_; }

private:
    size_t low_water_mark_;
    uint64_t generation_ = 1;
    std::vector<FeedItem> items_;
    std::set<std::wstring> seen_ids_;
    std::optional<size_t> current_index_;
    std::optional<uint64_t> last_dom_pos_;
    std::wstring last_video_id_;
    uint64_t duplicates_count_ = 0;
    uint64_t skipped_count_ = 0;
    uint64_t gaps_count_ = 0;
    bool exhausted_ = false;
};

}  // namespace bgtok
