#!/usr/bin/env python3
"""
Plot Gini coefficient comparison between progressive and flat fees.

Usage:
    python3 plot_gini.py [output_dir]

Reads gini_comparison.csv from output_dir (default: current directory)
and generates gini_comparison.png.
"""

import sys
import os

def main():
    # Check for required packages
    try:
        import pandas as pd
        import matplotlib.pyplot as plt
    except ImportError:
        print("Required packages not found. Install with:")
        print("  pip install pandas matplotlib")
        sys.exit(1)

    # Get output directory
    output_dir = sys.argv[1] if len(sys.argv) > 1 else "."

    # Read the comparison CSV
    comparison_path = os.path.join(output_dir, "gini_comparison.csv")
    if not os.path.exists(comparison_path):
        print(f"Error: {comparison_path} not found")
        print("Run the simulation first with:")
        print("  cargo run -p mc-cluster-tax --features cli --bin cluster-tax-sim -- compare")
        sys.exit(1)

    df = pd.read_csv(comparison_path)

    # Create figure with multiple subplots
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))
    fig.suptitle('Progressive vs Flat Transaction Fees: Impact on Wealth Distribution', fontsize=14, fontweight='bold')

    # Plot 1: Gini over time
    ax1 = axes[0, 0]
    ax1.plot(df['round'], df['gini_progressive'], 'b-', label='Progressive Fees', linewidth=2)
    ax1.plot(df['round'], df['gini_flat'], 'r--', label='Flat Fees', linewidth=2)
    ax1.set_xlabel('Simulation Round')
    ax1.set_ylabel('Gini Coefficient')
    ax1.set_title('Gini Coefficient Over Time')
    ax1.legend()
    ax1.grid(True, alpha=0.3)
    ax1.set_ylim(0, 1)

    # Plot 2: Gini difference
    ax2 = axes[0, 1]
    gini_diff = df['gini_flat'] - df['gini_progressive']
    ax2.fill_between(df['round'], 0, gini_diff, alpha=0.5,
                     color='green' if gini_diff.iloc[-1] > 0 else 'red',
                     label='Flat - Progressive')
    ax2.axhline(y=0, color='black', linestyle='-', linewidth=0.5)
    ax2.set_xlabel('Simulation Round')
    ax2.set_ylabel('Gini Difference')
    ax2.set_title('Inequality Reduction from Progressive Fees')
    ax2.legend()
    ax2.grid(True, alpha=0.3)

    # Plot 3: Read full data for fee analysis if available
    progressive_path = os.path.join(output_dir, "gini_progressive.csv")
    flat_path = os.path.join(output_dir, "gini_flat.csv")

    if os.path.exists(progressive_path) and os.path.exists(flat_path):
        prog_df = pd.read_csv(progressive_path)
        flat_df = pd.read_csv(flat_path)

        ax3 = axes[1, 0]
        quintiles = ['Q1\n(Poorest)', 'Q2', 'Q3', 'Q4', 'Q5\n(Richest)']

        # Get final fee rates by quintile
        prog_rates = [
            prog_df['q1_fee_rate'].iloc[-1],
            prog_df['q2_fee_rate'].iloc[-1],
            prog_df['q3_fee_rate'].iloc[-1],
            prog_df['q4_fee_rate'].iloc[-1],
            prog_df['q5_fee_rate'].iloc[-1],
        ]
        flat_rates = [
            flat_df['q1_fee_rate'].iloc[-1],
            flat_df['q2_fee_rate'].iloc[-1],
            flat_df['q3_fee_rate'].iloc[-1],
            flat_df['q4_fee_rate'].iloc[-1],
            flat_df['q5_fee_rate'].iloc[-1],
        ]

        x = range(len(quintiles))
        width = 0.35
        ax3.bar([i - width/2 for i in x], prog_rates, width, label='Progressive', color='blue', alpha=0.7)
        ax3.bar([i + width/2 for i in x], flat_rates, width, label='Flat', color='red', alpha=0.7)
        ax3.set_xlabel('Wealth Quintile')
        ax3.set_ylabel('Fee Rate (basis points)')
        ax3.set_title('Fee Rates by Wealth Quintile')
        ax3.set_xticks(x)
        ax3.set_xticklabels(quintiles)
        ax3.legend()
        ax3.grid(True, alpha=0.3, axis='y')

        # Plot 4: Top 1% and Top 10% wealth share over time
        ax4 = axes[1, 1]
        if 'top_1_pct_share' in prog_df.columns:
            ax4.plot(prog_df['round'], prog_df['top_1_pct_share'] * 100, 'b-', label='Top 1% (Progressive)', linewidth=2)
            ax4.plot(flat_df['round'], flat_df['top_1_pct_share'] * 100, 'r--', label='Top 1% (Flat)', linewidth=2)
            ax4.plot(prog_df['round'], prog_df['top_10_pct_share'] * 100, 'b-', alpha=0.5, label='Top 10% (Progressive)', linewidth=1)
            ax4.plot(flat_df['round'], flat_df['top_10_pct_share'] * 100, 'r--', alpha=0.5, label='Top 10% (Flat)', linewidth=1)
        ax4.set_xlabel('Simulation Round')
        ax4.set_ylabel('Wealth Share (%)')
        ax4.set_title('Wealth Concentration Over Time')
        ax4.legend()
        ax4.grid(True, alpha=0.3)
    else:
        # Hide unused plots
        axes[1, 0].text(0.5, 0.5, 'Full data not available', ha='center', va='center', transform=axes[1, 0].transAxes)
        axes[1, 1].text(0.5, 0.5, 'Full data not available', ha='center', va='center', transform=axes[1, 1].transAxes)

    plt.tight_layout()

    # Save figure
    output_path = os.path.join(output_dir, "gini_comparison.png")
    plt.savefig(output_path, dpi=150, bbox_inches='tight')
    print(f"Plot saved to: {output_path}")

    # Also save a simple version
    fig2, ax = plt.subplots(figsize=(10, 6))
    ax.plot(df['round'], df['gini_progressive'], 'b-', label='Progressive Fees', linewidth=2.5)
    ax.plot(df['round'], df['gini_flat'], 'r--', label='Flat Fees', linewidth=2.5)
    ax.set_xlabel('Simulation Round', fontsize=12)
    ax.set_ylabel('Gini Coefficient', fontsize=12)
    ax.set_title('Wealth Inequality Over Time: Progressive vs Flat Transaction Fees', fontsize=14, fontweight='bold')
    ax.legend(fontsize=11)
    ax.grid(True, alpha=0.3)
    ax.set_ylim(0, 1)

    # Add annotation with final values
    final_prog = df['gini_progressive'].iloc[-1]
    final_flat = df['gini_flat'].iloc[-1]
    reduction = (final_flat - final_prog) / final_flat * 100 if final_flat > 0 else 0

    annotation = f'Final Gini:\n  Progressive: {final_prog:.4f}\n  Flat: {final_flat:.4f}\n  Reduction: {reduction:.1f}%'
    ax.annotate(annotation, xy=(0.98, 0.98), xycoords='axes fraction',
                fontsize=10, ha='right', va='top',
                bbox=dict(boxstyle='round', facecolor='wheat', alpha=0.5))

    simple_path = os.path.join(output_dir, "gini_simple.png")
    plt.savefig(simple_path, dpi=150, bbox_inches='tight')
    print(f"Simple plot saved to: {simple_path}")

    # Show plot if running interactively
    try:
        plt.show()
    except:
        pass

if __name__ == "__main__":
    main()
