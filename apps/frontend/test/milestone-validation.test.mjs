import test from "node:test";
import assert from "node:assert/strict";
import {
  toMilestoneUnits, formatMilestoneUnits, validateMilestoneStages, validateMilestoneLink,
} from "../src/lib/milestone-escrow.ts";

test("milestone amounts are exact integer USDC stroops", () => {
  assert.equal(toMilestoneUnits("12.0000001"), 120000001n);
  assert.equal(toMilestoneUnits("0.0000001"), 1n);
  assert.equal(formatMilestoneUnits(123456789n), "12.3456789");
  assert.equal(formatMilestoneUnits(100000000n), "10");
  for (const bad of ["0", "-1", "1e3", "1.00000001", "Infinity", "12."]) {
    assert.throws(() => toMilestoneUnits(bad));
  }
});

test("validated project stages must have exact funds and future due dates", () => {
  const stages = [
    { title: "Design approval", deliverableUrl: "https://example.com/design", amount: "10.25", dueDate: "2099-01-01" },
    { title: "Final code", deliverableUrl: "", amount: "20.50", dueDate: "2099-02-01" },
  ];
  assert.equal(validateMilestoneStages(stages), 307500000n);
  assert.throws(() => validateMilestoneStages([]), /1–12/);
  assert.throws(() => validateMilestoneStages([...Array(13)].map(() => stages[0])), /1–12/);
  assert.throws(() => validateMilestoneStages([{ ...stages[0], dueDate: "2001-01-01" }]), /future/i);
  assert.throws(() => validateMilestoneStages([{ ...stages[0], deliverableUrl: "javascript:alert(1)" }]), /HTTPS/);
  assert.throws(() => validateMilestoneStages([{ ...stages[0], title: "" }]), /title is required/);
});

test("submitted proof links cannot contain javascript or insecure transport", () => {
  assert.equal(validateMilestoneLink(" https://example.com/proof "), "https://example.com/proof");
  assert.throws(() => validateMilestoneLink("javascript:alert(1)"));
  assert.throws(() => validateMilestoneLink("http://example.com/proof"));
  assert.throws(() => validateMilestoneLink(""));
});
