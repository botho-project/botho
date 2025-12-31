#!/usr/bin/env python3
"""
Generate figures for cluster-tax documentation.

Outputs PNG files suitable for website/docs inclusion.
"""

import os
import numpy as np
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches

# Set style for clean, professional figures
plt.style.use('seaborn-v0_8-whitegrid')
plt.rcParams['figure.figsize'] = (10, 6)
plt.rcParams['font.size'] = 12
plt.rcParams['axes.labelsize'] = 14
plt.rcParams['axes.titlesize'] = 16
plt.rcParams['legend.fontsize'] = 11
plt.rcParams['figure.dpi'] = 150

# Output directory
OUTPUT_DIR = "docs/figures"
os.makedirs(OUTPUT_DIR, exist_ok=True)


# =============================================================================
# FEE CURVE FUNCTIONS
# =============================================================================

def flat_fee_rate(w, max_w, rate=0.05):
    return np.full_like(w, rate, dtype=float)


def linear_fee_rate(w, max_w, r_min=0.01, r_max=0.15):
    ratio = np.minimum(1.0, w / max_w)
    return r_min + (r_max - r_min) * ratio


def three_segment_fee_rate(w, max_w, w1_frac=0.15, w2_frac=0.70,
                           r_poor=0.01, r_mid_start=0.02, r_mid_end=0.10, r_rich=0.15):
    w1 = max_w * w1_frac
    w2 = max_w * w2_frac

    result = np.zeros_like(w, dtype=float)

    # Poor segment
    poor_mask = w < w1
    result[poor_mask] = r_poor

    # Middle segment (linear interpolation)
    mid_mask = (w >= w1) & (w < w2)
    t = (w[mid_mask] - w1) / (w2 - w1)
    result[mid_mask] = r_mid_start + t * (r_mid_end - r_mid_start)

    # Rich segment
    rich_mask = w >= w2
    result[rich_mask] = r_rich

    return result


def sigmoid_fee_rate(w, max_w, r_min=0.01, r_max=0.15, steepness=5.0):
    w_mid = max_w * 0.5
    x = steepness * (w - w_mid) / max_w
    x = np.clip(x, -10, 10)
    sigmoid = 1.0 / (1.0 + np.exp(-x))
    return r_min + (r_max - r_min) * sigmoid


# =============================================================================
# FIGURE 1: FEE CURVE COMPARISON
# =============================================================================

def figure_fee_curves():
    """Compare fee curves across wealth levels."""
    print("Generating: Fee Curve Comparison...")

    max_w = 1_000_000
    w = np.linspace(0, max_w, 1000)

    fig, ax = plt.subplots(figsize=(10, 6))

    # Plot each curve
    ax.plot(w / 1000, flat_fee_rate(w, max_w) * 100,
            label='Flat 5%', color='#888888', linestyle='--', linewidth=2)
    ax.plot(w / 1000, linear_fee_rate(w, max_w) * 100,
            label='Linear 1%-15%', color='#2ecc71', linewidth=2)
    ax.plot(w / 1000, sigmoid_fee_rate(w, max_w) * 100,
            label='Sigmoid', color='#3498db', linewidth=2)
    ax.plot(w / 1000, three_segment_fee_rate(w, max_w) * 100,
            label='3-Segment (recommended)', color='#e74c3c', linewidth=2.5)

    # Mark segment boundaries
    w1 = max_w * 0.15 / 1000
    w2 = max_w * 0.70 / 1000
    ax.axvline(x=w1, color='#e74c3c', linestyle=':', alpha=0.5)
    ax.axvline(x=w2, color='#e74c3c', linestyle=':', alpha=0.5)

    # Labels
    ax.set_xlabel('Effective Wealth (thousands)')
    ax.set_ylabel('Fee Rate (%)')
    ax.set_title('Progressive Fee Curves: Comparison')
    ax.legend(loc='lower right')
    ax.set_xlim(0, 1000)
    ax.set_ylim(0, 18)

    # Add segment labels
    ax.text(w1/2, 16.5, 'Poor\n(flat)', ha='center', fontsize=9, color='#e74c3c')
    ax.text((w1+w2)/2, 16.5, 'Middle\n(linear)', ha='center', fontsize=9, color='#e74c3c')
    ax.text((w2+1000)/2, 16.5, 'Rich\n(flat)', ha='center', fontsize=9, color='#e74c3c')

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/fee_curves_comparison.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/fee_curves_comparison.png")


# =============================================================================
# FIGURE 2: GINI REDUCTION BAR CHART
# =============================================================================

def figure_gini_reduction():
    """Bar chart comparing Gini reduction across models."""
    print("Generating: Gini Reduction Comparison...")

    # Simulation results
    models = ['Flat 5%', 'Linear\n1%-15%', 'Sigmoid', '3-Segment\n(balanced)']
    gini_reduction = [0.2353, 0.2396, 0.2393, 0.2399]  # Absolute values (negative = good)
    burn_rates = [9.1, 12.8, 12.5, 12.4]
    colors = ['#888888', '#2ecc71', '#3498db', '#e74c3c']

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 5))

    # Gini reduction bars
    bars1 = ax1.bar(models, gini_reduction, color=colors, edgecolor='black', linewidth=1.2)
    ax1.set_ylabel('Gini Coefficient Reduction')
    ax1.set_title('Inequality Reduction by Fee Model')
    ax1.set_ylim(0.23, 0.245)

    # Add value labels
    for bar, val in zip(bars1, gini_reduction):
        ax1.text(bar.get_x() + bar.get_width()/2, bar.get_height() + 0.001,
                f'{val:.4f}', ha='center', va='bottom', fontsize=10)

    # Highlight best
    ax1.annotate('Best', xy=(3, 0.2399), xytext=(3, 0.243),
                ha='center', fontsize=10, color='#e74c3c',
                arrowprops=dict(arrowstyle='->', color='#e74c3c'))

    # Burn rate bars
    bars2 = ax2.bar(models, burn_rates, color=colors, edgecolor='black', linewidth=1.2)
    ax2.set_ylabel('Supply Burned (%)')
    ax2.set_title('Fee Burn Rate by Model')
    ax2.set_ylim(0, 16)

    # Add value labels
    for bar, val in zip(bars2, burn_rates):
        ax2.text(bar.get_x() + bar.get_width()/2, bar.get_height() + 0.3,
                f'{val:.1f}%', ha='center', va='bottom', fontsize=10)

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/gini_reduction_comparison.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/gini_reduction_comparison.png")


# =============================================================================
# FIGURE 3: PROVENANCE DECAY THROUGH COMMERCE
# =============================================================================

def figure_provenance_decay():
    """Visualize how source_wealth decays through legitimate commerce."""
    print("Generating: Provenance Decay Visualization...")

    # Simulate decay through commerce (from provenance_reference.py results)
    hops = list(range(11))
    source_wealth = [
        1_000_000,  # Initial
        683_333,    # Hop 1
        497_058,    # Hop 2
        376_881,    # Hop 3
        294_667,    # Hop 4
        236_145,    # Hop 5
        193_300,    # Hop 6
        161_282,    # Hop 7
        136_987,    # Hop 8
        118_338,    # Hop 9
        103_894,    # Hop 10
    ]

    # Also show pure 5% decay for comparison
    pure_decay = [1_000_000 * (0.95 ** i) for i in hops]

    fig, ax = plt.subplots(figsize=(10, 6))

    # Plot decay curves
    ax.plot(hops, [s/1000 for s in source_wealth],
            'o-', color='#e74c3c', linewidth=2.5, markersize=8,
            label='Commerce (mixing)')
    ax.plot(hops, [s/1000 for s in pure_decay],
            's--', color='#3498db', linewidth=2, markersize=6,
            label='Pure 5% decay (no mixing)')

    # Fill area between
    ax.fill_between(hops, [s/1000 for s in source_wealth], [s/1000 for s in pure_decay],
                    alpha=0.2, color='#e74c3c', label='Mixing benefit')

    # Reference line for "average" wealth
    ax.axhline(y=50, color='#2ecc71', linestyle=':', linewidth=2,
               label='Population average (50K)')

    ax.set_xlabel('Transaction Hops')
    ax.set_ylabel('Source Wealth (thousands)')
    ax.set_title('Provenance Decay: Whale Money Through Commerce')
    ax.legend(loc='upper right')
    ax.set_xlim(0, 10)
    ax.set_ylim(0, 1100)

    # Annotations
    ax.annotate('Whale origin\n(1M)', xy=(0, 1000), xytext=(1.5, 950),
                fontsize=10, ha='center')
    ax.annotate('After 10 hops:\n~104K (90% decay)', xy=(10, 104), xytext=(8, 250),
                fontsize=10, ha='center',
                arrowprops=dict(arrowstyle='->', color='#e74c3c'))

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/provenance_decay.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/provenance_decay.png")


# =============================================================================
# FIGURE 4: SPLIT RESISTANCE DIAGRAM
# =============================================================================

def figure_split_resistance():
    """Diagram showing that splitting doesn't reduce source_wealth."""
    print("Generating: Split Resistance Diagram...")

    fig, ax = plt.subplots(figsize=(10, 5))
    ax.set_xlim(0, 10)
    ax.set_ylim(0, 6)
    ax.axis('off')

    # Colors
    whale_color = '#e74c3c'
    utxo_color = '#3498db'

    # Before split - single large UTXO
    before_box = mpatches.FancyBboxPatch((0.5, 3.5), 3, 2,
                                          boxstyle="round,pad=0.1",
                                          facecolor=utxo_color, edgecolor='black', linewidth=2)
    ax.add_patch(before_box)
    ax.text(2, 4.5, 'UTXO', fontsize=14, ha='center', va='center', fontweight='bold', color='white')
    ax.text(2, 4.0, 'value: 1,000,000', fontsize=11, ha='center', va='center', color='white')
    ax.text(2, 3.7, 'source_wealth: 1,000,000', fontsize=10, ha='center', va='center', color='#ffcccc')

    ax.text(2, 2.8, 'BEFORE', fontsize=12, ha='center', fontweight='bold')

    # Arrow
    ax.annotate('', xy=(5.5, 4.5), xytext=(4, 4.5),
                arrowprops=dict(arrowstyle='->', lw=3, color='black'))
    ax.text(4.75, 5.2, 'SPLIT', fontsize=12, ha='center', fontweight='bold')

    # After split - multiple small UTXOs
    for i, y in enumerate([5.0, 4.0, 3.0, 2.0]):
        after_box = mpatches.FancyBboxPatch((6, y), 3.5, 0.8,
                                            boxstyle="round,pad=0.05",
                                            facecolor=utxo_color, edgecolor='black', linewidth=1.5)
        ax.add_patch(after_box)
        ax.text(7.75, y + 0.5, f'value: 250,000', fontsize=9, ha='center', va='center', color='white')
        ax.text(7.75, y + 0.2, f'source_wealth: 1,000,000', fontsize=8, ha='center', va='center', color='#ffcccc')

    ax.text(7.75, 1.3, 'AFTER (×4)', fontsize=12, ha='center', fontweight='bold')

    # Key insight box
    insight_box = mpatches.FancyBboxPatch((0.3, 0.3), 9.4, 0.8,
                                          boxstyle="round,pad=0.1",
                                          facecolor='#ffffcc', edgecolor='#cccc00', linewidth=2)
    ax.add_patch(insight_box)
    ax.text(5, 0.7, 'source_wealth is UNCHANGED by splitting → Fee rate stays the same!',
            fontsize=12, ha='center', va='center', fontweight='bold', color='#666600')

    ax.set_title('Split Resistance: Provenance Tags Defeat Fee Avoidance', fontsize=14, pad=20)

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/split_resistance.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/split_resistance.png")


# =============================================================================
# FIGURE 5: WHALE VS POOR FEE COMPARISON
# =============================================================================

def figure_whale_vs_poor():
    """Show fee difference between whale and poor for same transaction."""
    print("Generating: Whale vs Poor Fee Comparison...")

    fig, ax = plt.subplots(figsize=(8, 6))

    # Data
    categories = ['Whale\n(source: 1M)', 'Poor\n(source: 10K)']
    fee_rates = [15.0, 1.1]
    fee_amounts = [1500, 114]
    colors = ['#e74c3c', '#2ecc71']

    x = np.arange(len(categories))
    width = 0.5

    bars = ax.bar(x, fee_rates, width, color=colors, edgecolor='black', linewidth=2)

    ax.set_ylabel('Fee Rate (%)')
    ax.set_title('Same 10,000 Transfer, Different Source Wealth')
    ax.set_xticks(x)
    ax.set_xticklabels(categories)
    ax.set_ylim(0, 20)

    # Add fee amounts
    for bar, rate, amount in zip(bars, fee_rates, fee_amounts):
        ax.text(bar.get_x() + bar.get_width()/2, bar.get_height() + 0.5,
                f'{rate}%\n({amount:,} fee)', ha='center', va='bottom', fontsize=12, fontweight='bold')

    # Add ratio annotation
    ax.annotate('', xy=(1, 15), xytext=(0, 15),
                arrowprops=dict(arrowstyle='<->', color='black', lw=2))
    ax.text(0.5, 16.5, '13.2× higher', ha='center', fontsize=14, fontweight='bold')

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/whale_vs_poor.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/whale_vs_poor.png")


# =============================================================================
# FIGURE 6: SYSTEM OVERVIEW DIAGRAM
# =============================================================================

def figure_system_overview():
    """High-level system diagram."""
    print("Generating: System Overview...")

    fig, ax = plt.subplots(figsize=(12, 7))
    ax.set_xlim(0, 12)
    ax.set_ylim(0, 7)
    ax.axis('off')

    # Title
    ax.text(6, 6.7, 'Provenance-Based Progressive Fees', fontsize=18, ha='center', fontweight='bold')

    # Box style
    def add_box(x, y, w, h, text, subtext='', color='#3498db'):
        box = mpatches.FancyBboxPatch((x, y), w, h,
                                      boxstyle="round,pad=0.1",
                                      facecolor=color, edgecolor='black', linewidth=2)
        ax.add_patch(box)
        ax.text(x + w/2, y + h/2 + 0.15, text, fontsize=11, ha='center', va='center',
                fontweight='bold', color='white')
        if subtext:
            ax.text(x + w/2, y + h/2 - 0.25, subtext, fontsize=9, ha='center', va='center', color='white')

    # UTXO with tags
    add_box(0.5, 4, 2.5, 1.5, 'UTXO', 'value + source_wealth', '#3498db')

    # Fee calculation
    add_box(4, 4, 2.5, 1.5, 'Fee Calc', 'rate(source_wealth)', '#9b59b6')

    # Progressive rate
    add_box(7.5, 4, 2.5, 1.5, 'Progressive', '1% → 15%', '#e74c3c')

    # Burn
    add_box(4, 1.5, 2.5, 1.2, 'Fee Burned', 'Reduces supply', '#e67e22')

    # Arrows
    ax.annotate('', xy=(3.9, 4.75), xytext=(3.1, 4.75),
                arrowprops=dict(arrowstyle='->', lw=2))
    ax.annotate('', xy=(7.4, 4.75), xytext=(6.6, 4.75),
                arrowprops=dict(arrowstyle='->', lw=2))
    ax.annotate('', xy=(5.25, 3.9), xytext=(5.25, 2.8),
                arrowprops=dict(arrowstyle='->', lw=2))

    # Properties boxes
    props = [
        ('Split Resistant', 'Tags persist through splits'),
        ('Sybil Resistant', 'Tags inherited by recipients'),
        ('ZK Compatible', '3-segment OR-proofs'),
    ]

    for i, (title, desc) in enumerate(props):
        x = 0.5 + i * 3.7
        box = mpatches.FancyBboxPatch((x, 0.3), 3.2, 1,
                                      boxstyle="round,pad=0.05",
                                      facecolor='#ecf0f1', edgecolor='#bdc3c7', linewidth=1.5)
        ax.add_patch(box)
        ax.text(x + 1.6, 0.95, title, fontsize=10, ha='center', fontweight='bold')
        ax.text(x + 1.6, 0.55, desc, fontsize=9, ha='center', color='#666')

    plt.tight_layout()
    plt.savefig(f"{OUTPUT_DIR}/system_overview.png", dpi=150, bbox_inches='tight')
    plt.close()
    print(f"  Saved: {OUTPUT_DIR}/system_overview.png")


# =============================================================================
# MAIN
# =============================================================================

def main():
    print("=" * 60)
    print("GENERATING CLUSTER TAX DOCUMENTATION FIGURES")
    print("=" * 60)
    print(f"\nOutput directory: {OUTPUT_DIR}/\n")

    figure_fee_curves()
    figure_gini_reduction()
    figure_provenance_decay()
    figure_split_resistance()
    figure_whale_vs_poor()
    figure_system_overview()

    print("\n" + "=" * 60)
    print("ALL FIGURES GENERATED")
    print("=" * 60)

    # List all generated files
    print(f"\nGenerated files in {OUTPUT_DIR}/:")
    for f in sorted(os.listdir(OUTPUT_DIR)):
        if f.endswith('.png'):
            print(f"  - {f}")


if __name__ == "__main__":
    main()
